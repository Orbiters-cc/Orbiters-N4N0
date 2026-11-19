"""SBX-7: a steamvr.lock left by a killed controller with nothing to restore is cleared; a live holder's lock and a lock
whose session still needs its restore stay."""
import json
import os
import shutil
import time
import unittest
from unittest.mock import patch

import psutil
import sandbox_testbase  # first: every machine path into a temp folder
import sandbox

DEAD_PID = 0x7ffffff0   # no such process


class StaleLock(unittest.TestCase):
    def setUp(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True)
        sandbox.ROOT.mkdir(parents=True)

    def journal(self, session, restored):
        sandbox.atomic_json(sandbox.JOURNAL, {'session': session, 'pid': DEAD_PID, 'files': [], 'processes': [], 'restored': restored})

    def lock(self, session, pid=DEAD_PID, create_time=1.0):
        sandbox.LOCK.write_text(json.dumps({'session': session, 'pid': pid, 'owner_create_time': create_time, 'time': time.time()}))

    def test_dead_holder_before_its_journal_existed_is_cleared(self):
        self.journal('previous', restored=True)      # killed between the lock and its own journal
        self.lock('new')
        self.assertTrue(sandbox.clear_stale_lock('test'))
        self.assertFalse(sandbox.LOCK.exists())
        self.assertIn('stale steamvr.lock', sandbox.ORPHAN_LOG.read_text())

    def test_dead_holder_after_its_restore_is_cleared(self):
        self.journal('s', restored=True)            # killed between "restored" and the unlink
        self.lock('s')
        self.assertTrue(sandbox.clear_stale_lock('test'))

    def test_a_session_still_to_restore_keeps_its_lock(self):
        self.journal('s', restored=False)
        self.lock('s')
        self.assertFalse(sandbox.clear_stale_lock('test'))
        self.assertTrue(sandbox.LOCK.exists())

    def test_a_live_holder_keeps_its_lock(self):
        self.journal('previous', restored=True)
        me = psutil.Process()
        self.lock('new', pid=os.getpid(), create_time=me.create_time())
        self.assertFalse(sandbox.clear_stale_lock('test'))
        self.lock('new', pid=os.getpid(), create_time=me.create_time() + 5)   # same pid, another process
        self.assertTrue(sandbox.clear_stale_lock('test'))

    def test_an_old_format_lock_is_judged_by_its_time(self):
        self.journal('previous', restored=True)
        sandbox.LOCK.write_text(json.dumps({'session': 'x', 'pid': os.getpid(), 'time': time.time()}))
        self.assertFalse(sandbox.clear_stale_lock('test'), 'this process existed when the lock was written')
        sandbox.LOCK.write_text(json.dumps({'session': 'x', 'pid': os.getpid(), 'time': 1000.0}))
        self.assertTrue(sandbox.clear_stale_lock('test'), 'a process younger than the lock cannot hold it')

    def test_an_empty_lock_is_cleared_only_once_old(self):
        self.journal('previous', restored=True)
        sandbox.LOCK.write_text('')
        self.assertFalse(sandbox.clear_stale_lock('test'))
        self.assertTrue(sandbox.clear_stale_lock('test', now=time.time() + 600))

    def test_acquire_starts_a_session_after_clearing_and_waits_on_a_live_lock(self):
        self.journal('previous', restored=True)
        self.lock('new')
        started = []
        with patch.object(sandbox, 'processes', return_value=[]), patch.object(sandbox, 'owner_wants_vr', return_value=None), \
                patch.object(sandbox, 'start_controller', side_effect=lambda *a: (started.append(1), (_ for _ in ()).throw(RuntimeError('stop here')))[1]):
            with self.assertRaises(RuntimeError):
                sandbox.acquire_turn('t', wait_s=0, say=lambda *a, **k: None)
        self.assertTrue(started, 'a new session was started')
        self.lock('new', pid=os.getpid(), create_time=psutil.Process().create_time())
        with patch.object(sandbox, 'processes', return_value=[]), patch.object(sandbox, 'owner_wants_vr', return_value=None), \
                patch.object(sandbox, 'start_controller', side_effect=AssertionError('must wait')):
            with self.assertRaisesRegex(RuntimeError, 'lock exists'):
                sandbox.acquire_turn('t', wait_s=0, say=lambda *a, **k: None)


if __name__ == '__main__':
    unittest.main()
