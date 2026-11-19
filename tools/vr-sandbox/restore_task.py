"""The scheduled task "Orbiters VR sandbox restore" (SBX-1, SBX-8), reproducible from the repository.

The task used to run `pythonw <shared main checkout on H:>\\tools\\vr-sandbox\\sandbox.py restore-orphan` every 5 minutes:
it depended on H: being there, on that checkout's sandbox.py being complete at that second, and every failure was invisible.
Now:
- the restorer is a fixed copy of the sandbox's own files in `<sandbox root>\\restorer` (C:, beside the journal), refreshed at
  every session start (so it always reads the journal format of the session that wrote it) and by `install-restore-task`;
  a copy that does not compile or import is never installed over a working one;
- the task runs it every 5 minutes AND at logon (a power cut or reboot mid-session is repaired before the owner can start
  SteamVR), with `--task-log`: one line per run in `<sandbox root>\\restore-task.log`, kept under 1 MB.

Nothing here creates the task: `sandbox.py install-restore-task` writes the XML and prints the `schtasks` command for a
person to run.
"""
import os
from pathlib import Path
import py_compile
import shutil
import subprocess
import sys
from xml.sax.saxutils import escape

TASK_NAME = 'Orbiters VR sandbox restore'
FILES = ('sandbox.py', 'lease.py', 'vrchat_life.py', 'client_configs.py', 'restore_task.py', 'geometry.py')
LOG_LIMIT = 1024 * 1024


def install_restorer(source, dest, python=None):
    """Copies the sandbox's files into `dest` through a fresh folder: compiled and imported there first, then swapped in by
    rename. Returns `dest`; raises (leaving the installed copy as it was) when the new copy does not work."""
    source, dest = Path(source), Path(dest)
    new, old = dest.with_name(dest.name + '.new'), dest.with_name(dest.name + '.old')
    for leftover in (new, old):
        if leftover.exists():
            shutil.rmtree(leftover)
    new.mkdir(parents=True)
    for name in FILES:
        if (source / name).exists():
            shutil.copy2(source / name, new / name)
    try:
        for name in FILES:
            if (new / name).exists():
                py_compile.compile(str(new / name), doraise=True, cfile=str(new / '__pycache__' / (name + 'c')))
        check = subprocess.run([python or sys.executable, '-I', str(new / 'sandbox.py'), 'selftest'], capture_output=True, text=True,
                               timeout=60, env=dict(os.environ), creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        if check.returncode or 'selftest ok' not in check.stdout:
            raise RuntimeError('The new restorer copy does not start; the installed one is kept: ' + (check.stderr or check.stdout)[-800:])
    except BaseException:
        shutil.rmtree(new, ignore_errors=True)
        raise
    if dest.exists():
        os.replace(dest, old)
    os.replace(new, dest)
    shutil.rmtree(old, ignore_errors=True)
    return dest


def pythonw(python):
    """The windowless interpreter beside `python` (the task must not flash a console every 5 minutes)."""
    python = Path(python)
    candidate = python.with_name('pythonw.exe')
    return candidate if candidate.exists() else python


def task_xml(python, restorer, user):
    """The task definition: every 5 minutes and at `user`'s logon, as that user (interactive), one run at a time, at most
    10 minutes a run."""
    command = escape(str(pythonw(python)))
    arguments = escape(f'-I "{Path(restorer) / "sandbox.py"}" restore-orphan --task-log')
    user = escape(user)
    return f'''<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Puts SteamVR's files back after an Orbiters developer VR sandbox session that ended without its restore (tools/vr-sandbox/restore_task.py).</Description>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <Repetition>
        <Interval>PT5M</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>2026-11-19T00:00:00</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{escape(str(restorer))}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
'''


def write_task(path, python, restorer, user):
    """Writes the XML (UTF-16, as schtasks /XML expects) and returns the command that installs it."""
    Path(path).write_text(task_xml(python, restorer, user), encoding='utf-16')
    return f'schtasks /Create /TN "{TASK_NAME}" /XML "{path}" /F'


def log_run(log, line):
    """One line per task run; the log is cut to its newer half past 1 MB."""
    log = Path(log)
    log.parent.mkdir(parents=True, exist_ok=True)
    try:
        if log.stat().st_size > LOG_LIMIT:
            data = log.read_bytes()
            log.write_bytes(data[len(data) // 2:].split(b'\n', 1)[-1])
    except FileNotFoundError:
        pass
    with log.open('a', encoding='utf-8') as out:
        out.write(line.rstrip('\n') + '\n')
