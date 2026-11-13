"""VRChat's life in the sandbox: safe launch, clean close, the URL pipe, and Easy Anti-Cheat instrumentation.

Facts this module relies on (2026-11-13, see docs/vr-sandbox.md "Easy Anti-Cheat"):
- launch.exe first tries the named pipe \\\\.\\pipe\\VRChatURLLaunchPipe. When VRChat already runs, the
  vrchat://launch link goes to that running game and NO anti-cheat bootstrap happens. Otherwise it starts
  start_protected_game.exe (EAC bootstrapper), which installs/starts the EasyAntiCheat_EOS service and the
  EasyAntiCheat_EOSSys driver, starts VRChat.exe and exits.
- "Launch Error / EAC unload timeout" is a window of start_protected_game.exe; VRChat.exe never starts.
- The one occurrence seen (12:37:54) followed a VRChat that quit 10 s after start, while "Attempting to login to
  EOS", before "AntiCheat Session Begin". So the sandbox never closes a VRChat before its anti-cheat session began
  (or MIN_AGE_S passed), always waits for EAC to be fully idle before a launch, and logs every state change.
"""
import ctypes as C
import ctypes.wintypes as W
import json
import os
import re
import subprocess
import threading
import time
from pathlib import Path

import psutil

EAC_SERVICES = ('EasyAntiCheat_EOS', 'EasyAntiCheat_EOSSys')
GAME_NAMES = {'vrchat.exe', 'start_protected_game.exe', 'launch.exe', 'easyanticheat_eos.exe'}
PIPE = r'\\.\pipe\VRChatURLLaunchPipe'
VRCHAT_LOGS = Path(os.environ.get('USERPROFILE', '.')) / 'AppData/LocalLow/VRChat/VRChat'
MIN_AGE_S = 90              # never close a VRChat younger than this unless its anti-cheat session already began
CLOSE_WAIT_S = 30           # WM_CLOSE grace before a kill
IDLE_TIMEOUT_S = 180        # longest wait for EAC to unload before a launch
IDLE_MARGIN_S = 5           # EAC must stay idle this long before a launch
ERROR_WINDOW_AGE_S = 45     # a bootstrapper window older than this, with no VRChat.exe, is a launch error

user32 = C.WinDLL('user32', use_last_error=True)
EnumProc = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
user32.EnumWindows.argtypes = [EnumProc, W.LPARAM]
user32.GetWindowThreadProcessId.argtypes = [W.HWND, C.POINTER(W.DWORD)]
user32.IsWindowVisible.argtypes = [W.HWND]
user32.GetWindowTextW.argtypes = [W.HWND, W.LPWSTR, C.c_int]
user32.GetWindowRect.argtypes = [W.HWND, C.POINTER(W.RECT)]
user32.PostMessageW.argtypes = [W.HWND, W.UINT, W.WPARAM, W.LPARAM]
WM_CLOSE = 0x10


class Log:
    """Append-only JSON lines (eac-log.jsonl in the sandbox root): every launch, close and EAC state change."""
    def __init__(self, path):
        self.path = Path(path)
        self.lock = threading.Lock()

    def write(self, event, **data):
        record = {'t': round(time.time(), 3), 'at': time.strftime('%Y-%m-%d %H:%M:%S'), 'event': event, **data}
        with self.lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with self.path.open('a', encoding='utf-8') as out:
                out.write(json.dumps(record) + '\n')
        return record


def service_state(name):
    try:
        out = subprocess.run(['sc.exe', 'queryex', name], capture_output=True, text=True, timeout=10,
                             creationflags=subprocess.CREATE_NO_WINDOW).stdout
    except (OSError, subprocess.TimeoutExpired) as error:
        return {'state': 'UNKNOWN', 'error': str(error)}
    state = re.search(r'STATE\s+:\s+\d+\s+(\w+)', out)
    pid = re.search(r'PID\s+:\s+(\d+)', out)
    if not state:
        return {'state': 'ABSENT' if '1060' in out else 'UNKNOWN'}
    return {'state': state.group(1), 'pid': int(pid.group(1)) if pid else 0}


class _Entry(C.Structure):
    _fields_ = [('size', C.c_uint32), ('usage', C.c_uint32), ('pid', C.c_uint32), ('heap', C.c_size_t), ('module', C.c_uint32),
                ('threads', C.c_uint32), ('parent', C.c_uint32), ('priority', C.c_int32), ('flags', C.c_uint32), ('name', C.c_wchar * 260)]
_kernel = C.WinDLL('kernel32', use_last_error=True)
_kernel.CreateToolhelp32Snapshot.argtypes = [C.c_uint32, C.c_uint32]
_kernel.CreateToolhelp32Snapshot.restype = C.c_void_p
_kernel.Process32FirstW.argtypes = [C.c_void_p, C.POINTER(_Entry)]
_kernel.Process32NextW.argtypes = [C.c_void_p, C.POINTER(_Entry)]
_kernel.CloseHandle.argtypes = [C.c_void_p]


def game_processes(names=GAME_NAMES):
    """Names and parents from one native snapshot; creation times only for the few matches (psutil.process_iter
    asking every process took 19 s on this machine)."""
    handle = _kernel.CreateToolhelp32Snapshot(2, 0)
    if handle in (None, C.c_void_p(-1).value):
        raise C.WinError(C.get_last_error())
    matches = []
    try:
        entry = _Entry(); entry.size = C.sizeof(entry)
        ok = _kernel.Process32FirstW(handle, C.byref(entry))
        while ok:
            if entry.name.lower() in names:
                matches.append((entry.pid, entry.name, entry.parent))
            ok = _kernel.Process32NextW(handle, C.byref(entry))
    finally:
        _kernel.CloseHandle(handle)
    found = []
    for pid, name, parent in matches:
        try:
            found.append({'name': name, 'pid': pid, 'ppid': parent, 'create_time': psutil.Process(pid).create_time()})
        except psutil.Error:
            pass
    return sorted(found, key=lambda x: x['create_time'] or 0)


def windows_of(pid):
    result = []
    def visit(hwnd, _):
        owner = W.DWORD()
        user32.GetWindowThreadProcessId(hwnd, C.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            text = C.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, text, 256)
            rect = W.RECT()
            user32.GetWindowRect(hwnd, C.byref(rect))
            result.append({'hwnd': hwnd, 'title': text.value, 'rect': [rect.left, rect.top, rect.right, rect.bottom]})
        return True
    user32.EnumWindows(EnumProc(visit), 0)
    return result


def snapshot():
    procs = game_processes()
    return {'services': {name: service_state(name) for name in EAC_SERVICES},
            'processes': [{k: v for k, v in p.items()} for p in procs]}


def idle(snap):
    """EAC fully unloaded and no game, bootstrapper or launcher process left."""
    return not snap['processes'] and all(s['state'] in ('STOPPED', 'ABSENT') for s in snap['services'].values())


def driver_events(since_s=900):
    """FilterManager load(6)/unload(1) events of the EAC driver over the last since_s seconds (read-only)."""
    query = ("*[System[Provider[@Name='Microsoft-Windows-FilterManager'] and (EventID=1 or EventID=6) and "
             f"TimeCreated[timediff(@SystemTime) <= {int(since_s * 1000)}]]]")
    try:
        out = subprocess.run(['wevtutil', 'qe', 'System', '/q:' + query, '/f:text', '/rd:true', '/c:20'], capture_output=True,
                             text=True, timeout=15, creationflags=subprocess.CREATE_NO_WINDOW).stdout
    except (OSError, subprocess.TimeoutExpired) as error:
        return [{'error': str(error)}]
    events = []
    for block in out.split('Event[')[1:]:
        if 'EasyAntiCheat' not in block:
            continue
        date = re.search(r'Date:\s*(\S+)', block)
        events.append({'time': date.group(1) if date else None, 'id': int(re.search(r'Event ID:\s*(\d+)', block).group(1)),
                       'what': 'loaded' if 'successfully loaded' in block else 'unloaded'})
    return events


class Sampler:
    """Background sampler: records each change of EAC service/process state with its time, for a bounded window."""
    def __init__(self, log, label, seconds, interval=.5):
        self.log, self.label, self.deadline, self.interval = log, label, time.monotonic() + seconds, interval
        self.done = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        last = None
        while not self.done.is_set() and time.monotonic() < self.deadline:
            snap = snapshot()
            key = json.dumps({'s': {k: v['state'] for k, v in snap['services'].items()},
                              'p': sorted((p['name'], p['pid']) for p in snap['processes'])})
            if key != last:
                self.log.write('eac_state', label=self.label, **snap)
                last = key
            self.done.wait(self.interval)

    def stop(self):
        self.done.set()
        self.thread.join(5)


def capture_window(window, path):
    try:
        from PIL import ImageGrab
        ImageGrab.grab(bbox=tuple(window['rect']), all_screens=True).save(path)
        return str(path)
    except Exception as error:  # evidence only; never blocks recovery
        return f'capture failed: {error}'


def error_windows(min_age_s=ERROR_WINDOW_AGE_S, now=None):
    """Bootstrapper processes that kept a window for min_age_s while no VRChat.exe started after them: the EAC
    "Launch Error" dialog (the normal splash closes within seconds, when VRChat.exe starts)."""
    now = time.time() if now is None else now
    procs = game_processes()
    found = []
    for p in procs:
        if p['name'].lower() != 'start_protected_game.exe' or now - p['create_time'] < min_age_s:
            continue
        # VRChat.exe is the bootstrapper's child (seen 2026-11-13); a VRChat of another launch does not count.
        if any(q['name'].lower() == 'vrchat.exe' and q['ppid'] == p['pid'] and q['create_time'] >= p['create_time'] for q in procs):
            continue
        wins = windows_of(p['pid'])
        if wins:
            found.append({**p, 'windows': wins})
    return found


def close_error_windows(log, evidence_dir, min_age_s=ERROR_WINDOW_AGE_S):
    """Presses the dialog's Exit (WM_CLOSE), kills the bootstrapper if it stays, and logs the EAC state first."""
    handled = []
    for p in error_windows(min_age_s):
        shot = capture_window(p['windows'][0], Path(evidence_dir) / f"launch-error-{p['pid']}-{int(time.time())}.png")
        log.write('launch_error_window', process=p, picture=shot, driver_events=driver_events(), **snapshot())
        for w in p['windows']:
            user32.PostMessageW(w['hwnd'], WM_CLOSE, 0, 0)
        try:
            proc = psutil.Process(p['pid'])
            if proc.create_time() == p['create_time']:
                try:
                    proc.wait(10)
                except psutil.TimeoutExpired:
                    proc.kill()
                    proc.wait(10)
        except psutil.NoSuchProcess:
            pass
        handled.append({'pid': p['pid'], 'picture': shot})
        log.write('launch_error_closed', pid=p['pid'])
    return handled


def wait_idle(log, label, timeout=IDLE_TIMEOUT_S, margin=IDLE_MARGIN_S, evidence_dir=None, sleep=time.sleep, clock=time.monotonic):
    """Blocks until EAC is idle for `margin` seconds. Leftover error dialogs are closed. Raises on timeout, never
    returns early: a launch must not happen while anything of the previous run is still unloading."""
    began = clock()
    calm_since = None
    first = None
    while True:
        if evidence_dir is not None:
            close_error_windows(log, evidence_dir)
        snap = snapshot()
        first = first or snap
        if idle(snap):
            calm_since = calm_since if calm_since is not None else clock()
            if clock() - calm_since >= margin:
                log.write('eac_idle', label=label, waited_s=round(clock() - began, 1))
                return round(clock() - began, 1)
        else:
            calm_since = None
        if clock() - began > timeout:
            log.write('eac_busy_timeout', label=label, first=first, **snap, driver_events=driver_events())
            raise RuntimeError(f'Easy Anti-Cheat did not unload within {timeout} s; not launching VRChat. State: '
                               + json.dumps(snap) + '. If EasyAntiCheat_EOSSys stays STOP_PENDING only a reboot clears it '
                               '(reported by VRChat players); details in eac-log.jsonl.')
        sleep(.5)


def vrchat_log_for(pid_create_time):
    """The output log VRChat wrote for the process started at pid_create_time (newest log created after it)."""
    logs = sorted(VRCHAT_LOGS.glob('output_log_*.txt'), key=lambda p: p.stat().st_ctime)
    after = [p for p in logs if p.stat().st_ctime >= pid_create_time - 5]
    return after[0] if after else None


def anticheat_began(create_time):
    path = vrchat_log_for(create_time)
    if not path:
        return False
    try:
        return 'AntiCheat Session Begin' in path.read_text(encoding='utf-8', errors='replace')
    except OSError:
        return False


def close_vrchat(log, info, reason, sleep=time.sleep):
    """Closes the VRChat of `info` (pid + create_time) the way its own window close does, after its anti-cheat
    session began (or MIN_AGE_S), then kills it only if it did not quit within CLOSE_WAIT_S."""
    try:
        p = psutil.Process(info['pid'])
        if p.create_time() != info['create_time']:
            return {'closed': 'already gone'}
    except psutil.NoSuchProcess:
        return {'closed': 'already gone'}
    sampler = Sampler(log, 'close', 150)
    log.write('vrchat_close', reason=reason, pid=info['pid'], age_s=round(time.time() - info['create_time'], 1))
    waited = 0
    while not anticheat_began(info['create_time']) and time.time() - info['create_time'] < MIN_AGE_S and p.is_running():
        sleep(1); waited += 1
    if waited:
        log.write('vrchat_close_waited_for_anticheat', pid=info['pid'], waited_s=waited)
    how = 'window close'
    wins = windows_of(info['pid'])
    for w in wins:
        user32.PostMessageW(w['hwnd'], WM_CLOSE, 0, 0)
    try:
        p.wait(CLOSE_WAIT_S if wins else 0)
    except psutil.TimeoutExpired:
        how = 'killed after window close timed out' if wins else 'killed (no window)'
        p.kill()
        try:
            p.wait(15)
        except psutil.TimeoutExpired:
            pass
    except psutil.NoSuchProcess:
        pass
    log.write('vrchat_closed', pid=info['pid'], how=how)
    try:
        waited = wait_idle(log, 'after close', timeout=120)
        result = {'closed': how, 'eac_idle_after_s': waited}
    except RuntimeError as error:
        result = {'closed': how, 'eac_warning': str(error)}
    sampler.stop()
    return result


def send_url(link, timeout=5):
    """Hands a vrchat://launch link to the running VRChat through its own pipe, as launch.exe does: no new process,
    no anti-cheat start. Returns False when no VRChat listens."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            pipe = open(PIPE, 'r+b', buffering=0)
            break
        except OSError:
            if time.monotonic() > deadline:
                return False
            time.sleep(.2)
    with pipe:
        pipe.write(link.encode('utf-8'))
        answer = []
        reader = threading.Thread(target=lambda: answer.append(pipe.read(64)), daemon=True)
        reader.start()
        reader.join(3)
    return True


def joined(create_time, instance, since):
    """(joining_seen, finished_seen) for `instance` in that VRChat's log after wall-clock `since`."""
    path = vrchat_log_for(create_time)
    if not path:
        return False, False
    joining = finished = False
    for line in path.read_text(encoding='utf-8', errors='replace').splitlines():
        stamp = re.match(r'(\d{4})\.(\d\d)\.(\d\d) (\d\d):(\d\d):(\d\d)', line)
        if not stamp or time.mktime(tuple(map(int, stamp.groups())) + (0, 0, -1)) < int(since):
            continue
        if '[Behaviour] Joining ' + instance in line:
            joining, finished = True, False
        elif joining and 'Finished entering world' in line:
            finished = True
    return joining, finished


def wait_joined(create_time, instance, since, timeout=120, sleep=time.sleep):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        j, f = joined(create_time, instance, since)
        if f:
            return True
        sleep(1)
    return False
