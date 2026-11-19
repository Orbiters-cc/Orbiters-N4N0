"""SBX-8 (and SBX-1's logon trigger): the scheduled task's definition, its fixed restorer copy, and its per-run log.
Nothing here creates or changes a scheduled task."""
import json
import shutil
import subprocess
import sys
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import patch

import sandbox_testbase  # first: every machine path into a temp folder
import restore_task
import sandbox

NS = {'t': 'http://schemas.microsoft.com/windows/2004/02/mit/task'}
HERE = Path(sandbox.__file__).parent


class TaskDefinition(unittest.TestCase):
    def test_every_five_minutes_and_at_logon_running_the_fixed_copy(self):
        restorer = Path(r'C:\Users\someone\.orbiters-build\vr-sandbox\restorer')
        python = Path(sys.executable)
        root = ET.fromstring(restore_task.task_xml(python, restorer, r'PC\someone').split('\n', 1)[1])
        self.assertEqual(root.find('t:Triggers/t:TimeTrigger/t:Repetition/t:Interval', NS).text, 'PT5M')
        self.assertEqual(root.find('t:Triggers/t:LogonTrigger/t:UserId', NS).text, r'PC\someone')
        self.assertEqual(root.find('t:Principals/t:Principal/t:LogonType', NS).text, 'InteractiveToken')
        self.assertEqual(root.find('t:Settings/t:MultipleInstancesPolicy', NS).text, 'IgnoreNew')
        self.assertEqual(root.find('t:Settings/t:ExecutionTimeLimit', NS).text, 'PT10M')
        command = root.find('t:Actions/t:Exec/t:Command', NS).text
        self.assertEqual(Path(command).name.lower(), 'pythonw.exe' if python.with_name('pythonw.exe').exists() else python.name.lower())
        self.assertEqual(root.find('t:Actions/t:Exec/t:Arguments', NS).text,
                         '-I "' + str(restorer / 'sandbox.py') + '" restore-orphan --task-log')
        self.assertNotIn('H:', restore_task.task_xml(python, restorer, 'u'), 'nothing on the shared checkout')

    def test_the_command_for_the_person_and_the_xml_encoding(self):
        path = Path(sandbox_testbase.BASE) / 'task.xml'
        command = restore_task.write_task(path, sys.executable, Path('C:/r'), r'PC\u')
        self.assertEqual(command, f'schtasks /Create /TN "Orbiters VR sandbox restore" /XML "{path}" /F')
        self.assertTrue(path.read_bytes().startswith(b'\xff\xfe'), 'UTF-16 with BOM, as schtasks /XML reads it')

    def test_the_log_stays_small(self):
        log = Path(sandbox_testbase.BASE) / 'runs.log'
        with patch.object(restore_task, 'LOG_LIMIT', 1000):
            for i in range(200):
                restore_task.log_run(log, f'run {i:04d} ' + 'x' * 20)
        self.assertLess(log.stat().st_size, 2100)
        self.assertTrue(log.read_text().endswith('run 0199 ' + 'x' * 20 + '\n'))


class Restorer(unittest.TestCase):
    def setUp(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True)
        sandbox.ROOT.mkdir(parents=True)
        self.dest = sandbox.ROOT / 'restorer'

    def run_task(self):
        return subprocess.run([sys.executable, '-I', str(self.dest / 'sandbox.py'), 'restore-orphan', '--task-log'],
                              capture_output=True, text=True, timeout=60)

    def test_the_copy_runs_on_its_own_and_logs_every_run(self):
        restore_task.install_restorer(HERE, self.dest)
        self.assertTrue((self.dest / 'sandbox.py').exists() and (self.dest / 'client_configs.py').exists())
        done = self.run_task()
        self.assertEqual(done.returncode, 0, done.stderr)
        lines = (sandbox.ROOT / 'restore-task.log').read_text().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertIn('"nothing": "no journal"', lines[0]); self.assertIn(str(self.dest), lines[0])
        sandbox.JOURNAL.write_text('{broken')
        self.assertEqual(self.run_task().returncode, 0)
        self.assertIn('cannot be read', (sandbox.ROOT / 'restore-task.log').read_text().splitlines()[1])
        self.assertIn('NOT restored', sandbox.ORPHAN_LOG.read_text())

    def test_a_broken_copy_never_replaces_a_working_one(self):
        restore_task.install_restorer(HERE, self.dest)
        broken = Path(sandbox_testbase.BASE) / 'broken'
        shutil.rmtree(broken, ignore_errors=True)
        shutil.copytree(HERE, broken, ignore=shutil.ignore_patterns('__pycache__'))
        (broken / 'lease.py').write_text('def oops(:\n')
        with self.assertRaises(Exception):
            restore_task.install_restorer(broken, self.dest)
        self.assertEqual((self.dest / 'lease.py').read_bytes(), (HERE / 'lease.py').read_bytes())
        self.assertFalse(self.dest.with_name('restorer.new').exists())


class Commands(unittest.TestCase):
    def test_a_typo_is_refused_without_reaching_a_controller(self):
        with patch.object(sandbox.urllib.request, 'urlopen', side_effect=AssertionError('network')), \
                patch.object(sandbox, 'remote', side_effect=AssertionError('remote')):
            for typo in ('restore-orphanx', 'statsu', 'restor'):
                with self.assertRaises(SystemExit):
                    with patch('sys.stderr'):
                        sandbox.main([typo])


if __name__ == '__main__':
    unittest.main()
