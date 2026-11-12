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
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import threading
import time
import urllib.request
import psutil

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

def processes():
    return [p.info for p in psutil.process_iter(['pid', 'name', 'create_time'])
            if p.info['name'] and p.info['name'].lower() in NAMES]

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

def restore():
    if not JOURNAL.exists():
        return {'restored': False, 'reason': 'no recovery journal'}
    j = json.loads(JOURNAL.read_text())
    if j.get('restored'):
        return {'restored': True, 'already_restored': True}
    # Only stop exact process identities recorded by this session. Refuse a foreign VR session.
    owned = {(p['pid'], p['create_time']) for p in j['processes']}
    foreign = [p for p in processes() if (p['pid'], p['create_time']) not in owned]
    if foreign:
        raise RuntimeError('Restore refused while unowned VR processes run: ' + json.dumps(foreign))
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
    if remaining:
        raise RuntimeError('Owned processes did not exit after terminate: '+json.dumps([p.pid for p in remaining]))
    if processes():
        raise RuntimeError('VR processes remain; configuration retained for standalone restore')
    for entry in j['files']:
        path, saved = Path(entry['path']), Path(entry['backup'])
        if entry['tree']:
            if path.exists():
                shutil.rmtree(path)
            if entry['exists']:
                shutil.copytree(saved, path)
        elif entry['exists']:
            shutil.copy2(saved, path)
            if hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
                raise RuntimeError('Restore hash mismatch: ' + str(path))
        else:
            path.unlink(missing_ok=True)
    j['restored'] = True
    atomic_json(JOURNAL, j)
    # Do not release somebody else's lock.
    if LOCK.exists() and json.loads(LOCK.read_text())['session'] == j['session']:
        LOCK.unlink()
    return {'restored': True, 'verified_files': len(j['files'])}

class Session:
    def __init__(self):
        self.stop = threading.Event()
        self.guard = threading.Lock()
        self.mm = mmap.mmap(-1, C.sizeof(Shared), tagname='Local\\OrbitersVrSandbox1112')
        self.shared = Shared.from_buffer(self.mm)
        self.mutex = kernel.CreateMutexW(None, False, 'Local\\OrbitersVrSandbox1112Mutex')
        self.shared.magic, self.shared.version, self.shared.owner = 0x56523132, 1, os.getpid()
        for i, d in enumerate(self.shared.devices):
            d.p[:] = ([0, 1.65, 0], [-.25, 1.2, -.35], [.25, 1.2, -.35])[i]
            d.q[:] = [1, 0, 0, 0]
        self.j = None

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
        self.j = {'session': identity, 'pid': os.getpid(), 'created': time.time(), 'token': secrets.token_hex(32), 'files': [], 'processes': [], 'restored': False}
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
        subprocess.Popen([sys.executable, __file__, 'watch', str(os.getpid()), str(psutil.Process().create_time())], creationflags=subprocess.CREATE_NO_WINDOW)
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
        if action in ('capture','overlays','devices'):
            command=[str(ROOT/'probe.exe'),action]
            if action=='capture':
                prefix=Path(args['output_prefix']).resolve()
                prefix.parent.mkdir(parents=True,exist_ok=True)
                command.append(str(prefix))
            elif action=='overlays':
                command.extend(args.get('overlay_keys',[]))
            result=subprocess.run(command,capture_output=True,text=True,timeout=30,creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode:raise RuntimeError(result.stderr)
            return json.loads(result.stdout)
        slot = {'head':0,'left':1,'right':2}[args.get('device','head')]
        with self.guard:
            d = self.shared.devices[slot]
            if action == 'pose':
                target = args.get('position',list(d.p))
                q = args.get('rotation',list(d.q)) # quaternion w,x,y,z
                seconds = float(args.get('duration_s',0))
                if len(target)!=3 or len(q)!=4 or not all(math.isfinite(float(x)) for x in target+q) or not 0<=seconds<=30:
                    raise ValueError('finite position[3], quaternion[w,x,y,z], duration 0..30 required')
                norm = math.sqrt(sum(x*x for x in q))
                if norm<1e-8: raise ValueError('zero quaternion')
                q=[x/norm for x in q]
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
                if slot==0 or not math.isfinite(value) or not (-1 if index in [2,3] else 0)<=value<=1:raise ValueError('invalid controller input')
                if kernel.WaitForSingleObject(self.mutex,1000)!=0:raise RuntimeError('IPC mutex timeout')
                try:d.inputs[index]=value
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

def remote(args):
    j=json.loads(JOURNAL.read_text())
    req=urllib.request.Request(f'http://127.0.0.1:{PORT}/',data=json.dumps(args).encode(),headers={'Authorization':'Bearer '+j['token'],'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=40) as r:return json.load(r)

if __name__=='__main__':
    cmd=sys.argv[1]
    if cmd=='start':serve()
    elif cmd=='restore':print(json.dumps(restore()))
    elif cmd=='watch':
        identity={'pid':int(sys.argv[2]),'create_time':float(sys.argv[3])}
        while same_process(identity):time.sleep(.5)
        try:restore()
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
