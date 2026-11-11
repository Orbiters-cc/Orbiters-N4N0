"""Run after deskcargo builds example review; never targets an existing application."""
import ctypes
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
from ctypes import wintypes

binary = Path(sys.argv[1]).resolve(strict=True)
root = Path(tempfile.gettempdir()) / 'Orbiters-QR-Web-owned-v1'

def processes():
    script = "Get-CimInstance Win32_Process -Filter \"name='msedgewebview2.exe'\" | Select-Object ProcessId,ParentProcessId,CreationDate,CommandLine | ConvertTo-Json -Compress"
    raw = subprocess.check_output(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', script], text=True, creationflags=0x08000000).strip()
    value = json.loads(raw) if raw else []
    return value if isinstance(value, list) else [value]

def start(env=None, mode=None):
    child = subprocess.Popen([str(binary)] + ([mode] if mode else []), env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, creationflags=0x08000000)
    lines = queue.Queue()
    def read():
        for line in child.stdout:
            lines.put(line.strip())
    threading.Thread(target=read, daemon=True).start()
    return child, lines

def until(lines, predicate):
    deadline = time.monotonic() + 35
    while time.monotonic() < deadline:
        line = lines.get(timeout=max(.1, deadline - time.monotonic()))
        if predicate(line):
            return line
    raise AssertionError('driver output timed out')

def stop(child):
    if child.poll() is None:
        child.terminate()
    child.wait(timeout=10)

open_process = ctypes.windll.kernel32.OpenProcess
open_process.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
open_process.restype = wintypes.HANDLE
ctypes.windll.kernel32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
ctypes.windll.kernel32.CloseHandle.argtypes = [wintypes.HANDLE]

for mode in ['host-crash', 'browser-crash']:
    before = set(root.glob('session-*'))
    child, lines = start(mode='crash-child')
    try:
        until(lines, lambda line: line == 'ready')
        profiles = set(root.glob('session-*')) - before
        assert len(profiles) == 1, profiles
        rows = processes()
        owned = [r for r in rows if any(str(p).lower() in (r['CommandLine'] or '').lower() for p in profiles)]
        assert owned, 'no owned browser to crash'
        identities = {(r['ProcessId'], r['CreationDate']) for r in owned}
        if mode == 'host-crash':
            stop(child)
        else:
            browsers = [r for r in owned if '--type=' not in r['CommandLine']]
            assert len(browsers) == 1, browsers
            handle = open_process(1, False, browsers[0]['ProcessId'])
            assert handle, 'cannot open owned browser'
            try:
                assert ctypes.windll.kernel32.TerminateProcess(handle, 99)
            finally:
                ctypes.windll.kernel32.CloseHandle(handle)
        started = time.monotonic()
        while any(p.exists() for p in profiles):
            assert time.monotonic() - started < 30, 'profile cleanup timeout'
            time.sleep(.05)
        remaining = processes()
        assert not identities.intersection((r['ProcessId'], r['CreationDate']) for r in remaining), 'owned browser process survived'
        if mode == 'browser-crash':
            assert child.poll() is None, 'test host must survive browser crash'
        print(f'{mode}=passed cleanup_ms={int((time.monotonic()-started)*1000)}', flush=True)
    finally:
        stop(child)

for mode in ['profile-override', 'missing-runtime']:
    with tempfile.TemporaryDirectory(prefix='orbiters-web-review-fixture-') as directory:
        sentinel = Path(directory) / 'preserve.txt'
        sentinel.write_text('external fixture')
        env = os.environ.copy()
        if mode == 'profile-override':
            env['WEBVIEW2_USER_DATA_FOLDER'] = directory
        else:
            env['WEBVIEW2_BROWSER_EXECUTABLE_FOLDER'] = str(Path(directory) / 'nonexistent-runtime')
        before = set(root.glob('session-*'))
        child, lines = start(env=env)
        try:
            line = until(lines, lambda line: line.startswith('network failure='))
            if mode == 'profile-override':
                assert 'effective profile override rejected' in line, line
            assert sentinel.read_text() == 'external fixture'
            assert not (set(root.glob('session-*')) - before), 'failed startup left a profile'
            print(f'{mode}=passed {line}', flush=True)
        finally:
            stop(child)
