"""Developer SteamVR session controller; never installed with Orbiters.

python sandbox.py start | status | stop | restore
python sandbox.py call '{"action":"pose", "device":"head", "position":[0,1.6,0]}'
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

ROOT = Path(os.environ['USERPROFILE']) / '.orbiters-build/vr-sandbox'
LOCK = ROOT / 'steamvr.lock'
JOURNAL = ROOT / 'session.json'
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

def processes():
    result=[]
    for pid,name in process_names():
        if name.lower() in NAMES:
            try:result.append({'pid':pid,'name':name,'create_time':psutil.Process(pid).create_time()})
            except psutil.NoSuchProcess:pass
    return result

def same_process(info):
    try:
        p = psutil.Process(info['pid'])
        return p if p.create_time() == info['create_time'] else None
    except psutil.NoSuchProcess:
        return None

def atomic_json(path, value):
    tmp = path.with_suffix('.tmp')
    tmp.write_text(json.dumps(value, indent=2), encoding='utf-8')
    os.replace(tmp, path)

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
        try:remote({'action':'stop'},journal=j)
        except (OSError,urllib.error.URLError):pass # owner may already be closing its HTTP server
        deadline=time.monotonic()+45
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
    if j.get('app_test_exe'):
        subprocess.run([j['app_test_exe'],'--quit'],timeout=20,creationflags=subprocess.CREATE_NO_WINDOW,check=True)
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
    if remaining:
        raise RuntimeError('Owned processes did not exit after terminate: '+json.dumps([p.pid for p in remaining]))
    if processes():
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
    atomic_json(JOURNAL, j)
    if j.get('app_restore_exe'):
        args=[j['app_restore_exe']]+([] if j.get('app_was_visible') else ['--hidden'])
        subprocess.Popen(args,creationflags=subprocess.CREATE_NO_WINDOW)
    # Do not release somebody else's lock.
    if LOCK.exists() and json.loads(LOCK.read_text())['session'] == j['session']:
        LOCK.unlink()
    return {'restored': True, 'verified_files': len(j['files'])}

class Session:
    def __init__(self):
        self.stop = threading.Event()
        self.guard = threading.Lock()
        self.j = None

    def devices(self):
        self.mm = mmap.mmap(-1, C.sizeof(Shared), tagname='Local\\OrbitersVrSandbox1112')
        self.shared = Shared.from_buffer(self.mm)
        self.mutex = kernel.CreateMutexW(None, False, 'Local\\OrbitersVrSandbox1112Mutex')
        self.shared.magic, self.shared.version, self.shared.owner = 0x56523132, 1, os.getpid()
        for i, d in enumerate(self.shared.devices):
            d.p[:] = ([0, 1.65, 0], [-.25, 1.2, -.35], [.25, 1.2, -.35])[i]
            d.q[:] = [1, 0, 0, 0]

    def start(self):
        ROOT.mkdir(parents=True, exist_ok=True)
        running = processes()
        if running:
            raise RuntimeError('Person is using VR; no changes made: ' + json.dumps(running))
        if JOURNAL.exists() and not json.loads(JOURNAL.read_text()).get('restored'):
            raise RuntimeError('Unrestored session exists; run restore first')
        identity = secrets.token_hex(16)
        lock = {'session': identity, 'pid': os.getpid(), 'stream': 'vr1112/sandbox-camera', 'time': time.time()}
        with LOCK.open('x') as f:
            json.dump(lock, f)
        self.j = {'session': identity, 'pid': os.getpid(), 'owner_create_time':psutil.Process().create_time(), 'created': time.time(), 'token': secrets.token_hex(32), 'files': [], 'processes': [], 'restored': False}
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
        subprocess.Popen([sys.executable, __file__, 'watch', str(os.getpid()), str(psutil.Process().create_time()), self.j['session']], creationflags=subprocess.CREATE_NO_WINDOW)
        self.devices()
        config = json.loads(SETTINGS.read_text(encoding='utf-8-sig')) if SETTINGS.exists() else {}
        config.setdefault('steamvr', {}).update({'forcedDriver': 'orbiters_sandbox', 'forcedHmd': 'orbiters-sandbox-head', 'activateMultipleDrivers': True, 'enableHomeApp': False, 'supersampleScale': 1.0, 'showMirrorView': False})
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
        proc = subprocess.Popen([str(RUNTIME/'bin/win64/vrstartup.exe')], creationflags=subprocess.CREATE_NO_WINDOW)
        self.record()
        return proc

    def record(self):
        known = {(p['pid'],p['create_time']) for p in self.j['processes']}
        for p in processes():
            if (p['pid'],p['create_time']) not in known:
                if p['name'].lower()=='vrchat.exe':
                    link=self.j.get('launch_link')
                    if not link or any(x['name'].lower()=='vrchat.exe' for x in self.j['processes']):continue
                    try:
                        if link not in psutil.Process(p['pid']).cmdline():continue
                    except psutil.Error:continue
                self.j['processes'].append(p)
        atomic_json(JOURNAL, self.j)

    def call(self, args):
        action = args['action']
        if action == 'status':
            self.record()
            return {'running': processes(), 'devices': [{'position':list(d.p),'rotation':list(d.q),'inputs':list(d.inputs)} for d in self.shared.devices]}
        if action == 'stop':
            self.record()
            self.stop.set()
            return {'stopping':True}
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
            install=Path(os.environ['LOCALAPPDATA'])/'Programs/Orbiters - N4N0/orbiters-desktop.exe'
            if self.j.get('app_test_exe'):raise RuntimeError('App already replaced in this session')
            appdir=Path(os.environ['APPDATA'])/'Orbiters/Desktop'
            for path in [appdir/'settings.json',appdir/'vrchat-bridge.dat',Path(os.environ['USERPROFILE'])/'.codex/config.toml',Path(os.environ['USERPROFILE'])/'.claude.json']:
                backup(self.j,path)
            visible=any('--window' in p.cmdline() for p in psutil.process_iter(['name']) if p.info['name']=='orbiters-desktop.exe')
            self.j.update(app_test_exe=str(exe),app_restore_exe=str(install),app_was_visible=visible)
            atomic_json(JOURNAL,self.j)
            # --quit only signals the service. Wait for its existing processes to
            # exit before launching our binary, or single-instance arbitration can
            # silently discard it while the installed service is still shutting down.
            previous=[p for p in psutil.process_iter(['name']) if p.info['name']==install.name and Path(p.exe()).resolve()==install.resolve()]
            subprocess.run([str(install),'--quit'],timeout=20,creationflags=subprocess.CREATE_NO_WINDOW,check=True)
            _,alive=psutil.wait_procs(previous,timeout=20)
            if alive:raise RuntimeError('Installed Orbiters did not finish quitting; sandbox app was not launched')
            env=os.environ.copy();env['ORBITERS_VR_SANDBOX_ROOT']=str(Path(__file__).parent.resolve());env.pop('ORBITERS_NO_NETWORK_LISTENING',None)
            with (ROOT/'app-out.log').open('a') as out,(ROOT/'app-err.log').open('a') as err:
                proc=subprocess.Popen([str(exe),'--hidden'],env=env,stdout=out,stderr=err,creationflags=subprocess.CREATE_NO_WINDOW,cwd=exe.parent)
            return {'app_pid':proc.pid,'restore':str(install)}
        if action=='vrchat':
            if any(p['name'].lower()=='vrchat.exe' for p in processes()):raise RuntimeError('VRChat already running')
            world,instance,user=args['world_id'],args['instance'],args['user_id']
            if not re.fullmatch(r'wrld_[0-9a-f-]{36}',world) or not re.fullmatch(r'usr_[0-9a-f-]{36}',user):raise ValueError('invalid identity')
            if not re.fullmatch(r'[0-9]+~private\('+re.escape(user)+r'\)~region\((eu|us|use|jp)\)',instance):raise ValueError('only a strict private instance of this account')
            url=f'vrchat://launch?ref=orbiters.cc&id={world}:{instance}'
            self.j['launch_link']=url
            atomic_json(JOURNAL,self.j)
            launch=STEAM/'steamapps/common/VRChat/launch.exe'
            proc=subprocess.Popen([str(launch),url,'-screen-fullscreen','0','-screen-width','1280','-screen-height','720'],creationflags=subprocess.CREATE_NO_WINDOW)
            self.record()
            return {'launch_pid':proc.pid,'url':url,'mode':'SteamVR auto-detection; no --no-vr'}
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

def serve():
    s=Session()
    try:
        s.start()
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                if self.headers.get('Authorization')!='Bearer '+s.j['token']:
                    self.send_error(403);return
                try:
                    n=int(self.headers.get('Content-Length','0'))
                    if not 0<n<=65536:raise ValueError('body size')
                    answer=s.call(json.loads(self.rfile.read(n)))
                    data=json.dumps(answer).encode();self.send_response(200)
                except Exception as e:
                    data=json.dumps({'error':str(e)}).encode();self.send_response(400)
                self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        server=http.server.HTTPServer(('127.0.0.1',PORT),Handler)
        server.timeout=.25
        while not s.stop.is_set():
            server.handle_request();s.record()
        server.server_close()
    finally:
        if s.j and JOURNAL.exists():
            s.record();print(json.dumps(restore()),flush=True)

def remote(args,journal=None):
    j=json.loads(JOURNAL.read_text()) if journal is None else journal
    req=urllib.request.Request(f'http://127.0.0.1:{PORT}/',data=json.dumps(args).encode(),headers={'Authorization':'Bearer '+j['token'],'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=40) as r:return json.load(r)

if __name__=='__main__':
    cmd=sys.argv[1]
    if cmd=='start':serve()
    elif cmd=='restore':print(json.dumps(restore()))
    elif cmd=='watch':
        identity={'pid':int(sys.argv[2]),'create_time':float(sys.argv[3])}
        while same_process(identity):time.sleep(.5)
        try:restore(expected_session=sys.argv[4])
        except Exception as e:(ROOT/'restore-error.txt').write_text(str(e))
    elif cmd=='request':
        args=json.loads(sys.argv[2]);action=args['action']
        if action=='restore':answer=restore()
        elif action=='start':
            ROOT.mkdir(parents=True,exist_ok=True)
            with (ROOT/'server-out.log').open('a') as out,(ROOT/'server-err.log').open('a') as err:
                child=subprocess.Popen([sys.executable,__file__,'start'],stdout=out,stderr=err,creationflags=subprocess.CREATE_NO_WINDOW)
            deadline=time.monotonic()+45
            while True:
                if child.poll() is not None:raise RuntimeError((ROOT/'server-err.log').read_text())
                try:answer=remote({'action':'status'});break
                except Exception:
                    if time.monotonic()>deadline:raise RuntimeError('Sandbox startup timeout; inspect logs and restore')
                    time.sleep(.25)
        else:answer=remote(args)
        print(json.dumps(answer))
    else:print(json.dumps(remote(json.loads(sys.argv[2]) if cmd=='call' else {'action':cmd})))
