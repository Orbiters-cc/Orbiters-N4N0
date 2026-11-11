import base64, subprocess, tempfile, time, uuid, ctypes, os
from ctypes import wintypes
create_file=ctypes.windll.kernel32.CreateFileW
create_file.argtypes=[wintypes.LPCWSTR,wintypes.DWORD,wintypes.DWORD,wintypes.LPVOID,wintypes.DWORD,wintypes.DWORD,wintypes.HANDLE]
create_file.restype=wintypes.HANDLE
from pathlib import Path
source=(Path(__file__).resolve().parents[1]/'src'/'watcher.ps1').read_text()
root=Path(tempfile.gettempdir())/'Orbiters-QR-Web-owned-v1'
root.mkdir(exist_ok=True)
def wait(check):
    t=time.monotonic()
    while not check():
        assert time.monotonic()-t<20, 'watcher timed out'
        time.sleep(.05)
for mode in ['done','host-death','stale-sweep','delayed-unlock','bootstrap-orphan']:
    owned=root/('session-'+uuid.uuid4().hex);owned.mkdir();(owned/'owner.lock').touch()
    stale=root/('session-'+uuid.uuid4().hex)
    unrelated=root/('unrelated-'+uuid.uuid4().hex)
    if mode in ['stale-sweep','delayed-unlock','bootstrap-orphan']:
        stale.mkdir();(stale/'owner.lock').touch();unrelated.mkdir()
    if mode=='bootstrap-orphan':
        (stale/'owner.lock').unlink();os.utime(stale,(time.time()-60,time.time()-60))
    held=None
    if mode=='delayed-unlock':
        held=create_file(str(stale/'locked-file'),0xC0000000,0,None,4,0,None)
        assert held not in [None,ctypes.c_void_p(-1).value]
    owner=subprocess.Popen(['python','-c','import time;time.sleep(60)'],creationflags=0x08000000)
    script=source.replace('__ROOT__',str(root)).replace('__PATH__',str(owned)).replace('__PID__',str(owner.pid))
    watcher=subprocess.Popen(['powershell.exe','-NoProfile','-NonInteractive','-WindowStyle','Hidden','-EncodedCommand',base64.b64encode(script.encode('utf-16le')).decode()],creationflags=0x08000000)
    wait(lambda:(owned/'ready').exists())
    if mode in ['stale-sweep','delayed-unlock','bootstrap-orphan']:
        if held is not None:
            assert stale.exists(), 'lock fixture did not prevent deletion'
            ctypes.windll.kernel32.CloseHandle(wintypes.HANDLE(held))
        wait(lambda:not stale.exists());assert unrelated.exists();unrelated.rmdir()
    if mode=='host-death':owner.terminate();owner.wait()
    else:(owned/'done').touch()
    wait(lambda:not owned.exists());watcher.wait(timeout=10)
    if owner.poll() is None:owner.terminate();owner.wait()
    assert watcher.returncode==0
    print(mode+'=passed',flush=True)
