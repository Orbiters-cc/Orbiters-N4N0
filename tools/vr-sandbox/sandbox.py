"""Developer SteamVR session controller; never installed with Orbiters.

Shared session (default, see docs/vr-sandbox.md):
python sandbox.py acquire --owner vr1113b/shell [--app EXE] [--render 2112x2304|default] [--cold] [--wait 3600]
python sandbox.py call --owner vr1113b/shell '{"action":"pose", "device":"head", "position":[0,1.6,0]}'
python sandbox.py release --owner vr1113b/shell
python sandbox.py status | yield | restore
"""
import ctypes as C
import hashlib
import http.server
import json
import math
import mmap
import os
import re
from pathlib import Path
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.request
import psutil
import queue
sys.path.insert(0, str(Path(__file__).parent))
import lease
import vrchat_life

ROOT = Path(os.environ['USERPROFILE']) / '.orbiters-build/vr-sandbox'
LOCK = ROOT / 'steamvr.lock'
JOURNAL = ROOT / 'session.json'
LEASES = ROOT / 'leases'          # <owner>.json: that agent's turn token (written by the controller on grant)
STATE = ROOT / 'shared-session.json'  # public view of the shared session: holder, waiters, render, VRChat, history
YIELD = ROOT / 'YIELD'            # created by `sandbox.py yield` (or by hand): the owner wants SteamVR now
RENDER = ROOT / 'render-target.txt'
EAC_LOG = ROOT / 'eac-log.jsonl'
DEFAULT_RENDER = [1080, 1200]
HEADSET_HOLD_S = 10               # a Virtual Desktop headset connection must last this long before the session yields
NEUTRAL_POSES = ([0, 1.65, 0], [-.25, 1.2, -.35], [.25, 1.2, -.35])
STEAM = Path('C:/Program Files (x86)/Steam')
RUNTIME = STEAM / 'steamapps/common/SteamVR'
PATHS = Path(os.environ['LOCALAPPDATA']) / 'openvr/openvrpaths.vrpath'
SETTINGS = STEAM / 'config/steamvr.vrsettings'
NAMES = {'vrserver.exe', 'vrmonitor.exe', 'vrcompositor.exe', 'vrstartup.exe',
         'vrdashboard.exe', 'vrwebhelper.exe', 'vrprismhost.exe', 'vrchat.exe'}
PORT = 52912

class Device(C.Structure):
    _fields_ = [('p', C.c_double * 3), ('q', C.c_double * 4), ('inputs', C.c_float * 20)]
class Shared(C.Structure):
    _fields_ = [('magic', C.c_uint32), ('version', C.c_uint32), ('owner', C.c_uint32),
                ('reserved', C.c_uint32), ('devices', Device * 3)]
assert C.sizeof(Device) == 136 and C.sizeof(Shared) == 424
kernel = C.WinDLL('kernel32', use_last_error=True)
kernel.CreateMutexW.argtypes = [C.c_void_p, C.c_bool, C.c_wchar_p]
kernel.CreateMutexW.restype = C.c_void_p
kernel.WaitForSingleObject.argtypes = [C.c_void_p, C.c_uint32]
kernel.ReleaseMutex.argtypes = [C.c_void_p]
kernel.CloseHandle.argtypes = [C.c_void_p]
kernel.OpenProcess.argtypes = [C.c_uint32,C.c_bool,C.c_uint32]
kernel.OpenProcess.restype = C.c_void_p

def process_exited(pid):
    handle=kernel.OpenProcess(0x100000,False,pid)
    if not handle:return C.get_last_error()==87
    try:return kernel.WaitForSingleObject(handle,0)==0
    finally:kernel.CloseHandle(handle)

def terminate_dead_helper(p):
    # Only called for an exact journal-owned identity after the normal wait.
    result=subprocess.run(['taskkill','/PID',str(p.pid),'/F'],capture_output=True,text=True,timeout=10,creationflags=subprocess.CREATE_NO_WINDOW)
    return 'There is no running instance of the task' in result.stderr

def unit_rotation(values):
    norm=math.sqrt(sum(x*x for x in values))
    if norm<1e-8:raise ValueError('zero quaternion')
    q=[x/norm for x in values]
    # A literal zero W at a half-turn makes VRChat's native hand/camera snap,
    # while cos(pi/2) at the same orientation does not (live matrix-b4/b5).
    # Use the continuous trigonometric representation. The angular change is
    # below double precision at this orientation; the driver remains dev-only.
    if q[0]==0:q[0]=math.cos(math.pi/2)
    return q

class ProcessEntry(C.Structure):
    _fields_=[('size',C.c_uint32),('usage',C.c_uint32),('pid',C.c_uint32),('heap',C.c_size_t),('module',C.c_uint32),('threads',C.c_uint32),('parent',C.c_uint32),('priority',C.c_int32),('flags',C.c_uint32),('name',C.c_wchar*260)]
kernel.CreateToolhelp32Snapshot.argtypes=[C.c_uint32,C.c_uint32]
kernel.CreateToolhelp32Snapshot.restype=C.c_void_p
kernel.Process32FirstW.argtypes=[C.c_void_p,C.POINTER(ProcessEntry)]
kernel.Process32NextW.argtypes=[C.c_void_p,C.POINTER(ProcessEntry)]

def process_names():
    # Enumerate names natively before asking psutil for identities. Querying
    # every Windows process individually stalls each bridge request for seconds.
    handle=kernel.CreateToolhelp32Snapshot(2,0)
    if handle in (None,C.c_void_p(-1).value):raise C.WinError(C.get_last_error())
    try:
        entry=ProcessEntry();entry.size=C.sizeof(entry)
        if not kernel.Process32FirstW(handle,C.byref(entry)):raise C.WinError(C.get_last_error())
        result=[]
        while True:
            result.append((entry.pid,entry.name))
            if not kernel.Process32NextW(handle,C.byref(entry)):
                error=C.get_last_error()
                if error!=18:raise C.WinError(error)
                return result
    finally:kernel.CloseHandle(handle)

def process_dead(p):
    try:
        return p.status() == psutil.STATUS_ZOMBIE or p.num_threads() == 0 or p.num_handles() == 0 or process_exited(p.pid)
    except psutil.NoSuchProcess:
        return True

def processes():
    result=[]
    dead=[]
    if JOURNAL.exists():
        saved=json.loads(JOURNAL.read_text())
        dead=saved.get('dead_processes',[])
    for pid,name in process_names():
        if name.lower() in NAMES:
            try:
                p=psutil.Process(pid)
                info={'pid':pid,'name':name,'create_time':p.create_time()}
                if not process_dead(p) and not any(d['pid']==pid and d['create_time']==info['create_time'] for d in dead):result.append(info)
            except psutil.NoSuchProcess:pass
    return result

def same_process(info):
    try:
        p = psutil.Process(info['pid'])
        return p if p.create_time() == info['create_time'] and not process_dead(p) else None
    except psutil.NoSuchProcess:
        return None

def atomic_json(path, value):
    tmp = path.with_suffix('.tmp')
    tmp.write_text(json.dumps(value, indent=2), encoding='utf-8')
    try:
        for attempt in range(20):
            try:
                os.replace(tmp, path)
                return
            except PermissionError as error:
                # A concurrent journal reader can briefly deny DELETE sharing
                # on Windows. Keep the previous recovery intact while retrying.
                if getattr(error, 'winerror', None) not in (5, 32, 33) or attempt == 19:
                    raise
                time.sleep(.05)
    finally:
        tmp.unlink(missing_ok=True)

def backup(journal, path, tree=False):
    entry = {'path': str(path), 'tree': tree, 'exists': path.exists()}
    saved = ROOT / 'backup' / str(len(journal['files']))
    if path.exists():
        if tree:
            shutil.copytree(path, saved)
        else:
            saved.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, saved)
            entry['sha256'] = hashlib.sha256(saved.read_bytes()).hexdigest()
    entry['backup'] = str(saved)
    journal['files'].append(entry)

def tree_hashes(path):
    return {str(p.relative_to(path)):hashlib.sha256(p.read_bytes()).hexdigest() for p in path.rglob('*') if p.is_file()}

def restore(expected_session=None):
    if not JOURNAL.exists():
        return {'restored': False, 'reason': 'no recovery journal'}
    j = json.loads(JOURNAL.read_text())
    if expected_session is not None and j['session']!=expected_session:
        return {'restored':False,'reason':'session changed'}
    if j.get('restored'):
        return {'restored': True, 'already_restored': True}
    if j.get('pid') != os.getpid() and j.get('owner_create_time') and same_process({'pid':j['pid'],'create_time':j['owner_create_time']}):
        try:remote({'action':'shutdown','force':True,'reason':'restore'},journal=j)
        except (OSError,urllib.error.URLError):pass # owner may already be closing its HTTP server
        # A clean VRChat close waits for its anti-cheat session and for EAC to unload (vrchat_life.close_vrchat).
        deadline=time.monotonic()+420
        while time.monotonic()<deadline:
            latest=json.loads(JOURNAL.read_text())
            if latest['session']!=j['session']:return {'restored':False,'reason':'session changed'}
            if latest.get('restored'):return {'restored':True,'verified_files':len(latest['files'])}
            if not same_process({'pid':j['pid'],'create_time':j['owner_create_time']}):return restore_session(j['session'])
            time.sleep(.25)
        raise RuntimeError('Live sandbox did not finish restoration; inspect its error log')
    return restore_session(j['session'])

def restore_session(expected_session):
    # Owner shutdown is requested BEFORE taking this mutex: its finally block
    # must be able to restore while the caller waits for it. Watchdog and manual
    # retries serialize file mutation and re-read the journal under the mutex.
    mutex=kernel.CreateMutexW(None,False,'Local\\OrbitersVrSandboxRestore1112')
    if not mutex:raise C.WinError(C.get_last_error())
    acquired=False
    try:
        result=kernel.WaitForSingleObject(mutex,30000)
        if result not in (0,128):raise RuntimeError('Another restore did not finish within 30 seconds')
        acquired=True
        j=json.loads(JOURNAL.read_text())
        if j['session']!=expected_session:return {'restored':False,'reason':'session changed'}
        if j.get('restored'):return {'restored':True,'already_restored':True}
        return restore_contents(j)
    finally:
        if acquired:kernel.ReleaseMutex(mutex)
        kernel.CloseHandle(mutex)

def restore_contents(j):
    # Only stop exact process identities recorded by this session. Refuse a foreign VR session.
    owned = {(p['pid'], p['create_time']) for p in j['processes']}
    foreign = [p for p in processes() if (p['pid'], p['create_time']) not in owned]
    if foreign:
        raise RuntimeError('Restore refused while unowned VR processes run: ' + json.dumps(foreign))
    if j.get('turn_app'):stop_app(j['turn_app'])
    elif j.get('app_test_exe') and not j.get('shared'):
        subprocess.run([j['app_test_exe'],'--quit'],timeout=20,creationflags=subprocess.CREATE_NO_WINDOW,check=True)
    # VRChat first and cleanly: a game killed during its anti-cheat start-up is what preceded the only
    # "EAC unload timeout" seen; SteamVR going away first also makes VRChat quit on its own.
    log=vrchat_life.Log(EAC_LOG)
    for info in j['processes']:
        if info['name'].lower()=='vrchat.exe' and same_process(info):
            vrchat_life.close_vrchat(log,info,'session restore')
    terminating=[]
    for info in sorted(j['processes'], key=lambda p: p['name'].lower() != 'vrmonitor.exe'):
        p = same_process(info)
        if p:
            try:
                p.terminate()
                terminating.append(p)
            except psutil.NoSuchProcess:
                pass
    _,remaining=psutil.wait_procs(terminating,timeout=10)
    still_running={(p['pid'],p['create_time']) for p in processes()}
    remaining=[p for p in remaining if (p.pid,p.create_time()) in still_running]
    warnings=[p.pid for p in remaining if process_dead(p) or terminate_dead_helper(p)]
    remaining=[p for p in remaining if p.pid not in warnings]
    if remaining:
        raise RuntimeError('Owned processes did not exit after terminate: '+json.dumps([p.pid for p in remaining]))
    if [p for p in processes() if p['pid'] not in warnings]:
        raise RuntimeError('VR processes remain; configuration retained for standalone restore')
    for entry in j['files']:
        path, saved = Path(entry['path']), Path(entry['backup'])
        if entry['tree']:
            expected=tree_hashes(saved) if entry['exists'] else {}
            if path.exists():
                shutil.rmtree(path)
            if entry['exists']:
                shutil.copytree(saved, path)
                if tree_hashes(path)!=expected:raise RuntimeError('Restore tree hash mismatch: '+str(path))
        elif entry['exists']:
            shutil.copy2(saved, path)
            if hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
                raise RuntimeError('Restore hash mismatch: ' + str(path))
        else:
            path.unlink(missing_ok=True)
    j['restored'] = True
    j['dead_processes']=[info for info in j['processes'] if info['pid'] in warnings]
    atomic_json(JOURNAL, j)
    if j.get('app_restore_exe'):
        args=[j['app_restore_exe']]+([] if j.get('app_was_visible') else ['--hidden'])
        subprocess.Popen(args,creationflags=subprocess.CREATE_NO_WINDOW)
    # Do not release somebody else's lock.
    if LOCK.exists() and json.loads(LOCK.read_text())['session'] == j['session']:
        LOCK.unlink()
    return {'restored': True, 'verified_files': len(j['files']), 'leftover_dead_pids': warnings}

def stop_app(app,timeout=20):
    """Stops one test program of a turn: its own --quit, then a kill if it stays. Never another program."""
    if not app:return None
    p=same_process({'pid':app['pid'],'create_time':app['create_time']})
    if not p:return 'already gone'
    try:subprocess.run([app['exe'],'--quit'],timeout=timeout,creationflags=subprocess.CREATE_NO_WINDOW)
    except (OSError,subprocess.TimeoutExpired):pass
    try:
        p.wait(timeout);return 'quit'
    except psutil.TimeoutExpired:
        p.kill();p.wait(10);return 'killed after --quit timed out'
    except psutil.NoSuchProcess:return 'quit'

def private_address(ip):
    parts=ip.split('.')
    if len(parts)!=4:return ip.lower().startswith(('fe80','fd'))
    try:a,b=int(parts[0]),int(parts[1])
    except ValueError:return False
    return a==10 or (a==192 and b==168) or (a==172 and 16<=b<=31)

def headset_connected():
    """The owner's headset is on: Virtual Desktop's streamer holds a TCP connection to a device on the local network
    (its idle connection goes to VD's internet service, which is ignored). Not yet seen with a real headset
    connection (none was made while this was written, 2026-11-13); it errs towards yielding."""
    for info in vrchat_life.game_processes({'virtualdesktop.streamer.exe'}):
        try:
            for c in psutil.Process(info['pid']).connections('tcp'):
                if c.status=='ESTABLISHED' and c.raddr and private_address(c.raddr.ip):return True
        except psutil.Error:pass
    return False

def owner_wants_vr():
    if YIELD.exists():return 'yield requested ('+str(YIELD)+')'
    if headset_connected():return "the owner's headset connected through Virtual Desktop"
    return None

def current_render():
    try:
        w,h=(int(x) for x in RENDER.read_text().split()[:2])
        if 256<=w<=4096 and 256<=h<=4096:return [w,h]
    except (OSError,ValueError):pass
    return list(DEFAULT_RENDER)

def owner_file(owner):
    return LEASES/(re.sub(r'[^A-Za-z0-9._-]','_',owner)+'.json')

class Session:
    def __init__(self):
        self.stop = threading.Event()
        self.guard = threading.Lock()
        self.j = None
        self.lease = lease.new_state(time.time())
        self.eac = vrchat_life.Log(EAC_LOG)
        self.render = current_render()
        self.shutdown_reason = None
        self.headset_since = None
        self.vrchat_seen = None

    def devices(self):
        self.mm = mmap.mmap(-1, C.sizeof(Shared), tagname='Local\\OrbitersVrSandbox1112')
        self.shared = Shared.from_buffer(self.mm)
        self.mutex = kernel.CreateMutexW(None, False, 'Local\\OrbitersVrSandbox1112Mutex')
        self.shared.magic, self.shared.version, self.shared.owner = 0x56523132, 1, os.getpid()
        for i, d in enumerate(self.shared.devices):
            d.p[:] = NEUTRAL_POSES[i]
            d.q[:] = [1, 0, 0, 0]

    def start(self, stream='shared'):
        ROOT.mkdir(parents=True, exist_ok=True)
        running = processes()
        if running:
            raise RuntimeError('Person is using VR; no changes made: ' + json.dumps(running))
        if JOURNAL.exists() and not json.loads(JOURNAL.read_text()).get('restored'):
            raise RuntimeError('Unrestored session exists; run restore first')
        wants=owner_wants_vr()
        if wants:raise RuntimeError('Not starting: '+wants)
        identity = secrets.token_hex(16)
        # Same keys as before the shared session (older scripts read session/pid/stream/time); `shared` is new.
        lock = {'session': identity, 'pid': os.getpid(), 'stream': stream, 'time': time.time(), 'shared': True}
        with LOCK.open('x') as f:
            json.dump(lock, f)
        self.j = {'session': identity, 'pid': os.getpid(), 'owner_create_time':psutil.Process().create_time(), 'created': time.time(), 'token': secrets.token_hex(32), 'files': [], 'processes': [], 'restored': False, 'shared': True, 'render': self.render}
        # Keep old recoveries; every new session gets its own backup directory.
        old = ROOT / 'backup'
        if old.exists():
            old.rename(ROOT / ('backup-' + str(time.time_ns())))
        for path in [PATHS, SETTINGS, STEAM/'config/chaperone_info.vrchap', STEAM/'config/steamvr.vrappconfig', STEAM/'config/steamapps.vrmanifest']:
            backup(self.j, path)
        for path in [STEAM/'config/vrappconfig', Path(os.environ['LOCALAPPDATA'])/'Orbiters/SteamVR']:
            backup(self.j, path, tree=True)
        atomic_json(JOURNAL, self.j)
        # Recovery outlives the control server, including an abrupt crash.
        detached([sys.executable, __file__, 'watch', str(os.getpid()), str(psutil.Process().create_time()), self.j['session']])
        self.devices()
        config = json.loads(SETTINGS.read_text(encoding='utf-8-sig')) if SETTINGS.exists() else {}
        config.setdefault('steamvr', {}).update({'forcedDriver': 'orbiters_sandbox', 'forcedHmd': 'orbiters-sandbox-head', 'activateMultipleDrivers': True, 'enableHomeApp': False, 'supersampleScale': 1.0, 'showMirrorView': False})
        # A startup dashboard intercepts the emulated camera wheel and covers
        # the eye pictures. Disable it only in this journalled test session;
        # restore_contents restores the original settings bytes afterward.
        config.setdefault('dashboard', {})['enableDashboard'] = False
        config['driver_orbiters_sandbox'] = {'enable': True}
        for directory in (RUNTIME/'drivers').iterdir():
            if directory.is_dir():
                config.setdefault('driver_'+directory.name,{})['enable']=False
        # Prevent unrelated drivers and utility overlays from changing emulated poses.
        config.setdefault('steamvr', {})['activateMultipleDrivers'] = False
        atomic_json(SETTINGS, config)
        paths = json.loads(PATHS.read_text(encoding='utf-8-sig'))
        paths['external_drivers'] = [str(ROOT/'driver')]
        atomic_json(PATHS, paths)
        self.eac.write('session_start', session=identity, render=self.render)
        proc = subprocess.Popen([str(RUNTIME/'bin/win64/vrstartup.exe')], creationflags=subprocess.CREATE_NO_WINDOW)
        self.record()
        self.publish()
        return proc

    def record(self):
        known = {(p['pid'],p['create_time']) for p in self.j['processes']}
        for p in processes():
            if (p['pid'],p['create_time']) not in known:
                if p['name'].lower()=='vrchat.exe':
                    link=self.j.get('launch_link')
                    # One live VRChat per session; a new one after a crash or a cold relaunch is recorded again.
                    if not link or any(x['name'].lower()=='vrchat.exe' and same_process(x) for x in self.j['processes']):continue
                    try:
                        if link not in psutil.Process(p['pid']).cmdline():continue
                    except psutil.Error:continue
                self.j['processes'].append(p)
                known.add((p['pid'],p['create_time']))
        atomic_json(JOURNAL, self.j)

    def vrchat(self):
        """The session's live VRChat (journal identity), or None."""
        for info in reversed(self.j['processes']):
            if info['name'].lower()=='vrchat.exe' and same_process(info):return info
        return None

    # ---- shared session: turns ------------------------------------------------------------------------------
    def publish(self):
        now=time.time();v=self.vrchat() if self.j else None
        state={'session':self.j and self.j['session'],'controller_pid':os.getpid(),'updated':now,'render':self.render,
               'vrchat':v and {'pid':v['pid'],'age_s':round(now-v['create_time'])},'home':self.j and self.j.get('home_link'),
               'lease':lease.public(self.lease,now),'history':self.lease['history'][-20:]}
        try:atomic_json(STATE,state)
        except OSError:pass
        return state

    def grant_file(self,owner,token):
        LEASES.mkdir(parents=True,exist_ok=True)
        atomic_json(owner_file(owner),{'owner':owner,'token':token,'session':self.j['session'],'time':time.time()})

    def end_turn(self,reason,owner=None,token=None):
        holder=lease.end_turn(self.lease,time.time(),reason,owner,token)
        if not holder:return None
        report={'owner':holder['owner'],'reason':reason}
        if holder.get('app'):
            report['app']=stop_app(holder['app'])
            self.j.pop('turn_app',None);atomic_json(JOURNAL,self.j)
        report['neutral']=self.neutral()
        report['avatar_moved_since_launch']=self.j.get('moved',False)
        owner_file(holder['owner']).unlink(missing_ok=True)
        self.eac.write('turn_end',**report)
        # The idle countdown starts once the turn's clean-up is over, not when it began.
        if self.lease['holder'] is None:self.lease['idle_since']=time.time()
        self.publish()
        return report

    def neutral(self):
        """The known starting point of every turn: no input held, head and hands at their start poses, VRChat's camera
        off and not flying (OSC), and every VRChat menu closed, checked in VRChat's own log ("Immobile status changed
        to: True/False" follows its menus; a short left Y press toggles them). The avatar's place in the world is
        reported, not reset: a link sent to a running VRChat only opens its world page and waits for a click
        (2026-11-13 15:51, "Attempted to change instance ID for an existing instance"), and rejoining needs a relaunch,
        which is what this shared session avoids. `acquire --cold` gives a fresh spawn."""
        done={}
        with self.guard:
            if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
            try:
                for i,d in enumerate(self.shared.devices):
                    d.inputs[:]=[0]*20;d.p[:]=NEUTRAL_POSES[i];d.q[:]=[1,0,0,0]
            finally:kernel.ReleaseMutex(self.mutex)
        done['inputs_released']=True;done['poses']='start poses'
        v=self.vrchat()
        if not v:return {**done,'vrchat':'not running'}
        for address,payload in ((b'/usercamera/Mode\0\0\0\0',b',i\0\0'+struct.pack('>i',0)),(b'/usercamera/Flying\0\0',b',F\0\0')):
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as osc:osc.sendto(address+payload,('127.0.0.1',9000))
        done['osc']={'camera_mode':0,'camera_flying':False}
        done['menus']=self.close_menus(v)
        done['avatar_moved_since_launch']=self.j.get('moved',False)
        return done

    def close_menus(self,v,tries=3):
        state=vrchat_life.menu_open(v['create_time'])
        if state is None:return "unknown: no menu line in VRChat's log yet (none opened since launch)"
        presses=0
        while state and presses<tries:
            with self.guard:
                if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                try:self.shared.devices[1].inputs[14]=1
                finally:kernel.ReleaseMutex(self.mutex)
                self.stop.wait(.12)
                if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                try:self.shared.devices[1].inputs[14]=0
                finally:kernel.ReleaseMutex(self.mutex)
            presses+=1;time.sleep(1.5)
            state=vrchat_life.menu_open(v['create_time'])
        return ('closed' if not state else 'STILL OPEN')+f' (VRChat log; {presses} press(es) of left Y)'

    def end_session(self,reason):
        self.shutdown_reason=reason
        if self.lease['holder']:self.end_turn('shutdown')
        self.eac.write('session_end',reason=reason)
        self.j['ended_reason']=reason;atomic_json(JOURNAL,self.j)
        self.stop.set()

    def tick(self):
        """Every two seconds from the serve loop: stale turns, idle shutdown, the owner taking SteamVR back."""
        now=time.time()
        wants=owner_wants_vr()
        if wants and wants.startswith('the owner'):
            self.headset_since=self.headset_since or now
            if now-self.headset_since<HEADSET_HOLD_S:wants=None
        elif not wants:self.headset_since=None
        if wants:
            YIELD.unlink(missing_ok=True)
            self.j['yielded']=wants
            return self.end_session('yield: '+wants)
        if lease.stale(self.lease['holder'],now):
            self.end_turn('expired')
        if lease.idle_expired(self.lease,now):
            return self.end_session('idle %d min'%(lease.IDLE_S//60))
        v=self.vrchat()
        if self.vrchat_seen and not v:
            self.eac.write('vrchat_exited',pid=self.vrchat_seen['pid'],note='not closed by the sandbox (crash or closed in game)')
        self.vrchat_seen=v

    def handle(self,args):
        action=args.get('action')
        owner,token=args.pop('lease_owner',None),args.pop('lease_token',None)
        now=time.time()
        if action=='status':
            self.record()
            state=self.publish()
            return {**state,'running': processes(), 'devices': [{'position':list(d.p),'rotation':list(d.q),'inputs':list(d.inputs)} for d in self.shared.devices]}
        if action=='acquire':
            owner=args.get('owner') or owner
            if self.shutdown_reason:raise lease.LeaseError('Session is shutting down: '+self.shutdown_reason)
            for _ in range(2):
                result,detail=lease.acquire(self.lease,owner,now)
                if result=='wait' and lease.stale(self.lease['holder'],now):
                    self.end_turn('taken over by '+owner);now=time.time();continue
                break
            if result=='wait':
                self.publish();return {'granted':False,'reason':detail}
            self.grant_file(owner,detail);self.eac.write('turn_start',owner=owner)
            state=self.publish()
            return {'granted':True,'owner':owner,'token':detail,'render':self.render,'vrchat':state['vrchat'],'home':state['home'],'avatar_moved_since_launch':self.j.get('moved',False)}
        if action=='renew':
            lease.renew(self.lease,owner,token,now);return {'renewed':True}
        h=self.lease['holder']
        mine=bool(h) and (owner,token)==(h['owner'],h['token'])
        if action in ('release','stop') and mine:
            return {'released':True,**self.end_turn('released',owner,token)}
        if action=='stop':
            # Older callers ended the whole session with stop; now only a turn ends. With no turn held it is a shutdown.
            if h:raise lease.LeaseError(f"Not your turn: {h['owner']} holds the sandbox")
            self.end_session('stop without a turn');return {'stopping':True}
        if action=='release':
            raise lease.LeaseError('You hold no turn (it may have expired or been taken over)')
        if action=='shutdown':
            if h and not args.get('force') and not mine:
                raise lease.LeaseError(f"{h['owner']} holds the sandbox; only the holder (or --force for recovery) shuts it down")
            self.end_session(args.get('reason') or ('shutdown by '+(owner or 'force')));return {'stopping':True}
        if action=='yield':
            self.j['yielded']='yield command';self.end_session('yield command');return {'stopping':True,'yielded':True}
        lease.check(self.lease,owner,token,action,now)
        try:return self.call(args)
        finally:
            if self.lease['holder']:self.lease['holder']['renewed']=time.time()

    def call(self, args):
        action = args['action']
        if (action=='input' and args.get('input') in ('stick_x','stick_y') and args.get('value')) or action=='motion_capture':
            self.j['moved']=True   # stick locomotion moves the avatar in the world; the next turns are told
        if action == 'neutral':
            return self.neutral()
        if action == 'app_stop':
            h=self.lease['holder'];result=stop_app(h.get('app'));h['app']=None
            self.j.pop('turn_app',None);atomic_json(JOURNAL,self.j);self.publish()
            return {'app':result}
        if action in ('menu','flick','camera'):
            hand=args.get('device','right')
            if hand not in ('left','right'):raise ValueError('controller required')
            slot=1 if hand=='left' else 2
            def inputs(values):
                if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                try:
                    for index,value in values.items():self.shared.devices[slot].inputs[index]=value
                finally:kernel.ReleaseMutex(self.mutex)
            def flick(stick):
                if len(stick)!=2 or not all(math.isfinite(x) and -1<=x<=1 for x in stick):raise ValueError('stick requires two finite axes in -1..1')
                inputs({2:stick[0],3:stick[1]});self.stop.wait(.25);inputs({2:0,3:0});self.stop.wait(.3)
            button=14 if slot==1 else 10
            with self.guard:
                if action=='flick':flick(args['stick'])
                elif action=='camera':
                    inputs({button:0,2:0,3:0});self.stop.wait(.2)
                    inputs({button:1});self.stop.wait(.9)
                    flick([-.866,.5]);flick([-.951,.309])
                    inputs({button:0})
                else:
                    menu=args['menu']
                    if menu=='wheel':inputs({button:1});self.stop.wait(.8)
                    elif menu=='quick':inputs({button:1});self.stop.wait(.12);inputs({button:0})
                    elif menu=='close':inputs({button:0,2:0,3:0})
                    else:raise ValueError('menu must be quick, wheel or close')
            return {'commanded':action,'device':hand,'verify':'Inspect a composited eye capture before the next selection.'}
        if action in ('camera_fly','camera_mode'):
            if action=='camera_fly':
                if not isinstance(args.get('enabled'),bool):raise ValueError('enabled must be a boolean')
                address=b'/usercamera/Flying';value=args['enabled']
                payload=b',T\0\0' if value else b',F\0\0'
            else:
                value=args.get('mode')
                if type(value) is not int or not 0<=value<=6:raise ValueError('mode must be an integer 0..6')
                address=b'/usercamera/Mode';payload=b',i\0\0'+struct.pack('>i',value)
            address+=b'\0'*(4-len(address)%4)
            packet=address+payload
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as osc:osc.sendto(packet,('127.0.0.1',9000))
            return {'requested':value,'address':address.rstrip(b'\0').decode(),'verify':'Read OSCQuery and inspect the camera; a write is not proof.'}
        if action=='app':
            exe=Path(args['app_exe']).resolve()
            if not exe.is_file() or not exe.is_relative_to(ROOT):raise ValueError('app binary must be inside the sandbox root')
            holder=self.lease['holder']
            if holder.get('app') and same_process(holder['app']):raise RuntimeError('Your turn already runs '+holder['app']['exe']+'; app_stop it first')
            logs=Path(args.get('logs') or ROOT/'logs').resolve()
            real=(Path(os.environ['LOCALAPPDATA'])/'Orbiters').resolve()
            if logs==real or logs.is_relative_to(real):raise ValueError("logs must not be the owner's own Orbiters folder")
            install=Path(os.environ['LOCALAPPDATA'])/'Programs/Orbiters - N4N0/orbiters-desktop.exe'
            if not self.j.get('app_restore_exe'):
                # First test program of this session: journal the owner's app files and stop his installed app once.
                # It stays stopped between turns and comes back when the session ends.
                appdir=Path(os.environ['APPDATA'])/'Orbiters/Desktop'
                for path in [appdir/'settings.json',appdir/'vrchat-bridge.dat',Path(os.environ['USERPROFILE'])/'.codex/config.toml',Path(os.environ['USERPROFILE'])/'.claude.json']:
                    backup(self.j,path)
                visible=any('--window' in p.cmdline() for p in psutil.process_iter(['name']) if p.info['name']=='orbiters-desktop.exe')
                self.j.update(app_restore_exe=str(install),app_was_visible=visible)
                atomic_json(JOURNAL,self.j)
                # --quit only signals the service. Wait for its existing processes to
                # exit before launching our binary, or single-instance arbitration can
                # silently discard it while the installed service is still shutting down.
                previous=[p for p in psutil.process_iter(['name']) if p.info['name']==install.name and Path(p.exe()).resolve()==install.resolve()]
                if previous:subprocess.run([str(install),'--quit'],timeout=20,creationflags=subprocess.CREATE_NO_WINDOW,check=True)
                _,alive=psutil.wait_procs(previous,timeout=20)
                if alive:raise RuntimeError('Installed Orbiters did not finish quitting; sandbox app was not launched')
            env=os.environ.copy();env['ORBITERS_VR_SANDBOX_ROOT']=str(Path(__file__).parent.resolve());env.pop('ORBITERS_NO_NETWORK_LISTENING',None)
            # The bridge inside the test program calls sandbox.py with these, so its requests count as this turn's.
            env['ORBITERS_VR_SANDBOX_OWNER']=holder['owner'];env['ORBITERS_VR_SANDBOX_LEASE']=holder['token']
            # The test app's logs stay in the sandbox: the person's own Orbiters logs folder is never written by a test build.
            env['ORBITERS_SANDBOX_LOGS']=str(logs);logs.mkdir(parents=True,exist_ok=True)
            with (ROOT/'app-out.log').open('a') as out,(ROOT/'app-err.log').open('a') as err:
                proc=subprocess.Popen([str(exe),'--hidden'],env=env,stdout=out,stderr=err,creationflags=subprocess.CREATE_NO_WINDOW,cwd=exe.parent)
            app={'pid':proc.pid,'create_time':psutil.Process(proc.pid).create_time(),'exe':str(exe),'logs':str(logs)}
            holder['app']=app;self.j['turn_app']=app;self.j['app_test_exe']=str(exe);atomic_json(JOURNAL,self.j);self.publish()
            return {'app_pid':proc.pid,'restore':str(install),'logs':str(logs)}
        if action=='vrchat':
            world,instance,user=args['world_id'],args['instance'],args['user_id']
            if not re.fullmatch(r'wrld_[0-9a-f-]{36}',world) or not re.fullmatch(r'usr_[0-9a-f-]{36}',user):raise ValueError('invalid identity')
            if not re.fullmatch(r'[0-9]+~private\('+re.escape(user)+r'\)~region\((eu|us|use|jp)\)',instance):raise ValueError('only a strict private instance of this account')
            url=f'vrchat://launch?ref=orbiters.cc&id={world}:{instance}'
            if self.vrchat():
                raise RuntimeError('VRChat already runs in this shared session: use it. A link sent to a running VRChat only opens '
                                   'its world page (it does not travel), so another world or a fresh spawn needs '
                                   '`sandbox.py acquire --owner <you> --cold`.')
            if any(p['name'].lower()=='vrchat.exe' for p in processes()):raise RuntimeError('A VRChat this session did not start is running')
            return self.launch(url)
        if action=='motion_capture':
            seconds=float(args.get('duration_s',0));frames=args.get('frames',24)
            target=args.get('position');rotation=args.get('rotation')
            if args.get('device') not in ('head','left','right') or not 0<seconds<=30 or type(frames) is not int or not 3<=frames<=80:
                raise ValueError('motion capture requires device, duration 0..30 and frames 3..80')
            if target is not None and (len(target)!=3 or not all(math.isfinite(float(x)) for x in target)):raise ValueError('finite position[3] required')
            if rotation is not None and (len(rotation)!=4 or not all(math.isfinite(float(x)) for x in rotation) or sum(x*x for x in rotation)<1e-8):raise ValueError('finite nonzero quaternion[4] required')
            prefix=Path(args['output_prefix']).resolve();prefix.parent.mkdir(parents=True,exist_ok=True)
            if shutil.disk_usage(prefix.parent).free<512*1024*1024:raise RuntimeError('Insufficient capture space')
            with Path(str(prefix)+'-probe-error.log').open('w') as errors:
                child=subprocess.Popen([str(ROOT/'motion-probe.exe'),str(prefix),str(frames),str(seconds)],stdout=subprocess.PIPE,stderr=errors,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
                ready=queue.Queue()
                def drain():
                    ready.put(child.stdout.readline())
                    for _ in child.stdout:pass
                reader=threading.Thread(target=drain,daemon=True);reader.start()
                try:
                    if json.loads(ready.get(timeout=15))!={'ready':True}:raise RuntimeError('Motion capture did not become ready')
                    command={k:v for k,v in args.items() if k in ('device','position','rotation','duration_s')};command['action']='pose'
                    began=time.perf_counter();result=self.call(command);ended=time.perf_counter()
                    atomic_json(Path(str(prefix)+'-command.json'),{'command':command,'began_qpc':began,'ended_qpc':ended})
                    if child.wait(timeout=seconds+30):raise RuntimeError('Motion probe failed; inspect its error log')
                    reader.join(timeout=2)
                finally:
                    if child.poll() is None:child.terminate();child.wait(timeout=10)
            manifest=Path(str(prefix)+'-motion.jsonl')
            samples=[json.loads(line) for line in manifest.read_text().splitlines()]
            return {'motion':result,'manifest':str(manifest),'samples':len(samples),'command':str(prefix)+'-command.json'}
        if action in ('capture','overlays','devices','view'):
            command=[str(ROOT/'probe.exe'),action]
            if action=='capture':
                prefix=Path(args['output_prefix']).resolve()
                prefix.parent.mkdir(parents=True,exist_ok=True)
                command.append(str(prefix))
            elif action=='overlays':
                keys=args.get('overlay_keys')
                if keys is None:
                    source=(Path(__file__).parents[2]/'crates/orbiters-desktop/src/overlay/orbit/zones.rs').read_text(encoding='utf-8')
                    enum=source.split('pub enum Zone {',1)[1].split('\n}',1)[0]
                    keys=['cc.orbiters.desktop.orbit.'+name for name in re.findall(r'^\s+([A-Z]\w*),',enum,re.M)]
                command.extend(keys)
            result=subprocess.run(command,capture_output=True,text=True,timeout=30,creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode:raise RuntimeError(result.stderr)
            data=json.loads(result.stdout)
            if action=='capture':return {'eyes':data,'paths':[str(prefix)+'-left.png',str(prefix)+'-right.png'],'timestamp':time.time()}
            return data
        slot = {'head':0,'left':1,'right':2}[args.get('device','head')]
        with self.guard:
            d = self.shared.devices[slot]
            if action == 'pose':
                target = args.get('position',list(d.p))
                q = args.get('rotation',list(d.q)) # quaternion w,x,y,z
                seconds = float(args.get('duration_s',0))
                if len(target)!=3 or len(q)!=4 or not all(math.isfinite(float(x)) for x in target+q) or not 0<=seconds<=30:
                    raise ValueError('finite position[3], quaternion[w,x,y,z], duration 0..30 required')
                q=unit_rotation(q)
                p0,q0=list(d.p),list(d.q)
                if sum(a*b for a,b in zip(q,q0))<0:q=[-x for x in q]
                start=time.monotonic()
                while True:
                    t=1 if not seconds else min(1,(time.monotonic()-start)/seconds)
                    t=t*t*(3-2*t)
                    r=[a+(b-a)*t for a,b in zip(q0,q)]; n=math.sqrt(sum(x*x for x in r))
                    if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                    try:d.p[:]=[a+(b-a)*t for a,b in zip(p0,target)];d.q[:]=[x/n for x in r]
                    finally:kernel.ReleaseMutex(self.mutex)
                    if t>=1 or self.stop.is_set():break
                    time.sleep(1/144)
                return {'position':list(d.p),'rotation':list(d.q)}
            if action == 'input':
                names=['trigger','grip','stick_x','stick_y','trigger_touch','grip_touch','stick_click','stick_touch','a','a_touch','b','b_touch','x','x_touch','y','y_touch','menu','menu_touch','thumbrest','trigger_click']
                index=names.index(args['input']);value=float(args['value'])
                duration=float(args.get('duration_s',0))
                if slot==0 or not math.isfinite(value) or not (-1 if index in [2,3] else 0)<=value<=1 or not 0<=duration<=10:raise ValueError('invalid controller input/duration')
                if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                try:
                    d.inputs[index]=value
                    if index==0:d.inputs[19]=float(value>=.55);d.inputs[4]=float(value>0)
                finally:kernel.ReleaseMutex(self.mutex)
                if duration:
                    self.stop.wait(duration)
                    if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                    try:
                        d.inputs[index]=0
                        if index==0:d.inputs[19]=0;d.inputs[4]=0
                    finally:kernel.ReleaseMutex(self.mutex)
                return {'device':args['device'],'input':args['input'],'value':value}
        raise ValueError('unknown action')

    def launch(self,url):
        """Cold VRChat start through its EAC launcher, only once anti-cheat is fully idle; the launch is watched
        and a "Launch Error" window of the bootstrapper is detected, logged with the EAC state, closed, and retried
        once after EAC is idle again."""
        launcher=STEAM/'steamapps/common/VRChat/launch.exe'
        error=None
        for attempt in (1,2):
            waited=vrchat_life.wait_idle(self.eac,'before launch',evidence_dir=ROOT)
            self.j['launch_link']=url;self.j['home_link']=url;atomic_json(JOURNAL,self.j)
            sampler=vrchat_life.Sampler(self.eac,'launch',120)
            began=time.time()
            self.eac.write('vrchat_launch',attempt=attempt,eac_idle_wait_s=waited,instance=url.split('id=',1)[1],steamvr=any(p['name'].lower()=='vrserver.exe' for p in processes()))
            proc=subprocess.Popen([str(launcher),url,'-screen-fullscreen','0','-screen-width','1280','-screen-height','720'],creationflags=subprocess.CREATE_NO_WINDOW)
            error=None
            try:
                while time.time()-began<120:
                    self.record()
                    v=self.vrchat()
                    if v:
                        self.eac.write('vrchat_started',pid=v['pid'],after_s=round(v['create_time']-began,1))
                        self.vrchat_seen=v
                        self.j['moved']=False;atomic_json(JOURNAL,self.j)
                        return {'launch_pid':proc.pid,'vrchat_pid':v['pid'],'url':url,'mode':'SteamVR auto-detection; no --no-vr','attempt':attempt}
                    error=vrchat_life.close_error_windows(self.eac,ROOT)
                    if error:break
                    time.sleep(1)
            finally:sampler.stop()
            if not error:
                self.eac.write('vrchat_launch_timeout',**vrchat_life.snapshot())
                raise RuntimeError('VRChat did not start within 120 s; EAC state in '+str(EAC_LOG))
            self.eac.write('vrchat_launch_failed',attempt=attempt,windows=error)
            time.sleep(30)
        raise RuntimeError('EAC showed "Launch Error" twice (pictures: '+json.dumps(error)+'); state in '+str(EAC_LOG))

def serve(owner=None,stream='shared'):
    s=Session()
    try:
        s.start(stream)
        if owner:
            result,token=lease.acquire(s.lease,owner,time.time())
            if result=='granted':s.grant_file(owner,token);s.eac.write('turn_start',owner=owner);s.publish()
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                if self.headers.get('Authorization')!='Bearer '+s.j['token']:
                    self.send_error(403);return
                try:
                    n=int(self.headers.get('Content-Length','0'))
                    if not 0<n<=65536:raise ValueError('body size')
                    answer=s.handle(json.loads(self.rfile.read(n)))
                    data=json.dumps(answer).encode();self.send_response(200)
                except Exception as e:
                    data=json.dumps({'error':str(e),'kind':type(e).__name__}).encode();self.send_response(400)
                self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        server=http.server.HTTPServer(('127.0.0.1',PORT),Handler)
        server.timeout=.25
        last=0
        while not s.stop.is_set():
            server.handle_request()
            if time.monotonic()-last>=2:
                last=time.monotonic();s.record();s.tick()
        server.server_close()
    finally:
        if s.j and JOURNAL.exists():
            s.record();print(json.dumps(restore()),flush=True)
            s.lease['holder']=None;s.publish()

def lease_args(owner=None):
    """(owner, token) for a request: the test program's environment, else that owner's saved token."""
    env_owner=os.environ.get('ORBITERS_VR_SANDBOX_OWNER')
    owner=owner or env_owner
    token=os.environ.get('ORBITERS_VR_SANDBOX_LEASE') if owner and owner==env_owner else None
    if owner and not token:
        try:token=json.loads(owner_file(owner).read_text())['token']
        except (OSError,ValueError,KeyError):pass
    return owner,token

def remote(args,journal=None,owner=None,timeout=None):
    j=json.loads(JOURNAL.read_text()) if journal is None else journal
    args=dict(args)
    if args.get('action')!='status':
        o,t=lease_args(owner)
        if o:args.setdefault('lease_owner',o)
        if t:args.setdefault('lease_token',t)
    slow={'app':90,'release':120,'stop':120,'acquire':180,'vrchat':480,'neutral':60,'app_stop':60}
    req=urllib.request.Request(f'http://127.0.0.1:{PORT}/',data=json.dumps(args).encode(),headers={'Authorization':'Bearer '+j['token'],'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=timeout or slow.get(args.get('action'),40)) as r:return json.load(r)
    except urllib.error.HTTPError as error:
        body=error.read()
        try:message=json.loads(body)['error']
        except Exception:message=body.decode(errors='replace')
        raise RuntimeError(message) from None

def controller_alive():
    """The live controller's journal, or None."""
    try:j=json.loads(JOURNAL.read_text())
    except (OSError,ValueError):return None
    if j.get('restored') or not j.get('owner_create_time'):return None
    return j if same_process({'pid':j['pid'],'create_time':j['owner_create_time']}) else None

def detached(command,out=None,err=None):
    """Starts `command` outside the caller's process tree and job: the shared controller and its watcher must outlive
    the agent that happened to start them (on 2026-11-13 a stopped agent task took a live session down with it, its
    controller being that task's child). A short-lived python in between exits at once, so a tree kill of the caller
    cannot reach them. Returns that program's pid."""
    helper=[sys.executable,__file__,'spawn',json.dumps({'command':command,'out':out,'err':err})]
    result=subprocess.run(helper,capture_output=True,text=True,timeout=30,creationflags=subprocess.CREATE_NO_WINDOW)
    if result.returncode:raise RuntimeError('Could not start detached: '+result.stderr[-800:])
    return int(result.stdout.strip())

def spawn(spec):
    flags=0x00000008|0x00000200   # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    out=open(spec['out'],'a') if spec.get('out') else subprocess.DEVNULL
    err=open(spec['err'],'a') if spec.get('err') else subprocess.DEVNULL
    try:child=subprocess.Popen(spec['command'],stdout=out,stderr=err,stdin=subprocess.DEVNULL,creationflags=flags|0x01000000)  # + BREAKAWAY_FROM_JOB
    except OSError:child=subprocess.Popen(spec['command'],stdout=out,stderr=err,stdin=subprocess.DEVNULL,creationflags=flags)  # the job forbids breakaway
    print(child.pid,flush=True)

def start_controller(owner=None):
    ROOT.mkdir(parents=True,exist_ok=True)
    command=[sys.executable,__file__,'start']+(['--owner',owner] if owner else [])
    with (ROOT/'server-err.log').open('a') as err:err.write('\n'+time.strftime('%Y-%m-%d %H:%M:%S')+' start '+(owner or '')+'\n')
    child=psutil.Process(detached(command,str(ROOT/'server-out.log'),str(ROOT/'server-err.log')))
    deadline=time.monotonic()+45
    while True:
        if not child.is_running() or child.status()==psutil.STATUS_ZOMBIE:raise RuntimeError('Sandbox did not start: '+(ROOT/'server-err.log').read_text()[-1500:])
        try:return remote({'action':'status'})
        except Exception:
            if time.monotonic()>deadline:raise RuntimeError('Sandbox startup timeout; inspect logs and restore')
            time.sleep(.25)

def wait_gone(j,timeout=480):
    deadline=time.monotonic()+timeout
    while same_process({'pid':j['pid'],'create_time':j['owner_create_time']}):
        if time.monotonic()>deadline:raise RuntimeError('Session did not finish shutting down; inspect server-err.log')
        time.sleep(.5)

def release_turn(owner):
    return remote({'action':'release'},owner=owner)

def parse_render(text):
    if text in (None,''):return None
    if text=='default':return list(DEFAULT_RENDER)
    w,h=(int(x) for x in text.lower().replace(' ','x').split('x'))
    if not (256<=w<=4096 and 256<=h<=4096):raise ValueError('render must be WxH within 256..4096')
    return [w,h]

def write_render(size):
    if size==DEFAULT_RENDER:RENDER.unlink(missing_ok=True)
    else:RENDER.write_text(f'{size[0]} {size[1]}\n')

def acquire_turn(owner,wait_s=3600,render=None,cold=False,app=None,logs=None,say=print):
    """Takes a turn on the shared session, starting the session if none runs. Waits (polling) for its turn and for
    the owner's own VR use to end. A different render size or cold=True restarts SteamVR and VRChat: the driver
    reads the render size once, when SteamVR loads it."""
    deadline=time.monotonic()+wait_s
    last_reason=None
    while True:
        reason=None
        j=controller_alive()
        if j is None:
            foreign=processes()
            wants=owner_wants_vr()
            if foreign:reason='VR is in use outside the shared session (the owner, or an older sandbox script): '+', '.join(sorted({p['name'] for p in foreign}))
            elif wants:reason='Not starting: '+wants
            elif LOCK.exists():reason='A sandbox lock exists without a live shared controller (an older script, or a restore in progress): '+LOCK.read_text()
            else:
                if render:write_render(render)
                try:
                    start_controller(owner)
                    render=cold=None  # a fresh session already has them
                    continue
                except RuntimeError as error:reason=str(error)
        elif not j.get('shared'):
            reason='An older sandbox script holds SteamVR (stream '+json.loads(LOCK.read_text()).get('stream','?')+'); waiting for its restore'
        else:
            try:answer=remote({'action':'acquire','owner':owner},journal=j,owner=owner)
            except (OSError,urllib.error.URLError) as error:answer={'granted':False,'reason':'controller busy: '+str(error)}
            except RuntimeError as error:answer={'granted':False,'reason':str(error)}
            if answer.get('granted'):
                if cold or (render and answer['render']!=render):
                    say(json.dumps({'restarting':'cold start requested' if cold else f"render {answer['render']} -> {render}"}),flush=True)
                    remote({'action':'shutdown','reason':'cold restart for '+owner},journal=j,owner=owner)
                    wait_gone(j)
                    cold=False
                    continue
                if app:
                    answer['app']=remote({'action':'app','app_exe':app,**({'logs':logs} if logs else {})},journal=j,owner=owner)
                answer.pop('token',None)
                return answer
            reason=answer['reason']
        if reason!=last_reason:say(json.dumps({'waiting':reason}),flush=True);last_reason=reason
        if time.monotonic()>deadline:raise RuntimeError('No turn within %d s: %s'%(wait_s,reason))
        time.sleep(5)

def main(argv):
    if argv and argv[0]=='spawn':spawn(json.loads(argv[1]));return
    if argv and argv[0]=='watch':
        identity={'pid':int(argv[1]),'create_time':float(argv[2])}
        while same_process(identity):time.sleep(.5)
        try:restore(expected_session=argv[3])
        except Exception as e:(ROOT/'restore-error.txt').write_text(str(e))
        return
    import argparse
    parser=argparse.ArgumentParser(prog='sandbox.py')
    parser.add_argument('command')
    parser.add_argument('json',nargs='?')
    parser.add_argument('--owner')
    parser.add_argument('--wait',type=float,default=3600)
    parser.add_argument('--render')
    parser.add_argument('--cold',action='store_true')
    parser.add_argument('--app')
    parser.add_argument('--logs')
    parser.add_argument('--force',action='store_true')
    a=parser.parse_args(argv)
    cmd=a.command
    if cmd=='start':serve(owner=a.owner,stream=a.owner or 'shared');return
    if cmd=='acquire':
        if not a.owner:raise SystemExit('acquire needs --owner <stream>')
        print(json.dumps(acquire_turn(a.owner,a.wait,parse_render(a.render),a.cold,a.app,a.logs)));return
    if cmd=='yield':
        YIELD.parent.mkdir(parents=True,exist_ok=True);YIELD.write_text(time.strftime('%Y-%m-%d %H:%M:%S'))
        j=controller_alive()
        if j:
            try:remote({'action':'yield'},journal=j)
            except Exception:pass
            wait_gone(j)
        YIELD.unlink(missing_ok=True)
        print(json.dumps({'yielded':True,'vr_processes_left':[p['name'] for p in processes()]}));return
    if cmd=='restore':
        j=controller_alive()
        if j and not a.force:
            holder=remote({'action':'status'},journal=j)['lease']['holder']
            if holder and holder['owner']!=a.owner:
                raise SystemExit(f"{holder['owner']} holds the shared session: end your own turn with `release --owner`, "
                                 "or wait. restore --force is for recovery only.")
        print(json.dumps(restore()));return
    if cmd=='status':
        j=controller_alive()
        print(json.dumps(remote({'action':'status'},journal=j) if j else {'session':None,'lock':LOCK.read_text() if LOCK.exists() else None,'vr_processes':processes()}));return
    if cmd=='release':print(json.dumps(release_turn(a.owner)));return
    if cmd in ('renew','shutdown','neutral','app_stop'):
        args={'action':cmd}
        if a.force:args['force']=True
        print(json.dumps(remote(args,owner=a.owner)));return
    if cmd=='call':print(json.dumps(remote(json.loads(a.json),owner=a.owner)));return
    if cmd=='request':
        # The bridge's path (crates/orbiters-desktop/src/bridge/sandbox.rs): owner and token come from the test
        # program's environment, set by the `app` action.
        args=json.loads(a.json);action=args['action']
        if action=='restore':answer=restore()
        elif action=='start':answer=acquire_turn(lease_args()[0] or a.owner or 'mcp',wait_s=30)
        else:answer=remote(args)
        print(json.dumps(answer));return
    print(json.dumps(remote({'action':cmd},owner=a.owner)))

if __name__=='__main__':main(sys.argv[1:])
