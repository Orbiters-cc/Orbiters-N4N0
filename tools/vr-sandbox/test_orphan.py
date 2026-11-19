"""SBX-1: a session that dies unrestored is retried until restored, every refusal is written down with its reason, and no
new session starts over it. No SteamVR: processes() is patched, every path lives under the temp test base."""
import json
import shutil
import unittest
from unittest.mock import patch

import sandbox_testbase  # first: every machine path into a temp folder
import sandbox

VRSERVER = {'pid': 4242, 'create_time': 7.0, 'name': 'vrserver.exe'}
DESKTOP_VRCHAT = {'pid': 4343, 'create_time': 8.0, 'name': 'VRChat.exe'}


class Orphan(unittest.TestCase):
    def setUp(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True)
        sandbox.ROOT.mkdir(parents=True)
        self.target = sandbox.ROOT.parent / 'steamvr.vrsettings'
        self.target.write_bytes(b'original')
        j = {'session': 'dead1', 'pid': 1, 'owner_create_time': 1.0, 'files': [], 'processes': [], 'restored': False, 'shared': True}
        sandbox.backup(j, self.target)
        sandbox.atomic_json(sandbox.JOURNAL, j)
        sandbox.LOCK.write_text(json.dumps({'session': 'dead1'}))
        self.target.write_bytes(b'forced sandbox')

    def log(self):
        return sandbox.ORPHAN_LOG.read_text(encoding='utf-8') if sandbox.ORPHAN_LOG.exists() else ''

    def test_a_refusal_is_written_down_and_a_later_run_heals_it(self):
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[VRSERVER]):
            for _ in range(3):
                self.assertFalse(sandbox.restore_orphan()['restored'])
        self.assertEqual(self.target.read_bytes(), b'forced sandbox', 'never restored while SteamVR runs')
        self.assertEqual(self.log().count('NOT restored'), 3)
        self.assertIn('unowned VR processes', self.log())
        marker = sandbox.MARKER.read_text(encoding='utf-8')
        self.assertIn('session dead1', marker); self.assertIn('vrserver.exe', marker); self.assertIn('close SteamVR', marker)
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            self.assertTrue(sandbox.restore_orphan()['restored'])
        self.assertEqual(self.target.read_bytes(), b'original')
        self.assertFalse(sandbox.MARKER.exists())
        self.assertIn('session dead1 restored', self.log())
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            self.assertEqual(sandbox.restore_orphan(), {'nothing': 'restored'})
        self.assertEqual(self.log().count('\n'), 4, 'nothing to do writes nothing')

    def test_a_desktop_vrchat_alone_does_not_block_the_files(self):
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[DESKTOP_VRCHAT]):
            self.assertTrue(sandbox.restore_orphan()['restored'])
        self.assertEqual(self.target.read_bytes(), b'original')
        with patch.object(sandbox, 'processes', return_value=[DESKTOP_VRCHAT, VRSERVER]):
            self.assertEqual(sandbox.blocking([DESKTOP_VRCHAT, VRSERVER]), [DESKTOP_VRCHAT, VRSERVER])

    def test_a_live_session_is_never_touched(self):
        with patch.object(sandbox, 'same_process', return_value=object()), patch.object(sandbox, 'processes', return_value=[]):
            self.assertEqual(sandbox.restore_orphan(), {'nothing': 'live session'})
        self.assertEqual(self.target.read_bytes(), b'forced sandbox')

    def test_an_unreadable_journal_is_written_down_not_a_crash(self):
        sandbox.JOURNAL.write_bytes(b'')
        result = sandbox.restore_orphan()
        self.assertFalse(result['restored'])
        self.assertIn('NOT restored', self.log())
        self.assertTrue(sandbox.MARKER.exists())

    def test_the_watcher_keeps_trying_with_back_off_until_restored(self):
        answers = [RuntimeError('Restore refused while unowned VR processes run: vrserver.exe')] * 4 + [{'restored': True}]
        sleeps = []
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'restore', side_effect=answers):
            self.assertEqual(sandbox.watch({'pid': 1, 'create_time': 1.0}, 'dead1', sleep=sleeps.append), {'restored': True})
        self.assertEqual(sleeps, [5, 10, 20, 40])
        self.assertEqual(self.log().count('watcher: session dead1 NOT restored'), 4)

    def test_the_watcher_leaves_when_another_session_took_over(self):
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'restore', return_value={'restored': False, 'reason': 'session changed'}):
            self.assertEqual(sandbox.watch({'pid': 1, 'create_time': 1.0}, 'dead1', sleep=lambda _: self.fail('no wait')), {'restored': False, 'reason': 'session changed'})

    def test_acquire_starts_no_session_over_an_unrestored_one(self):
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[VRSERVER]):
            sandbox.restore_orphan()
        said = []
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]), \
                patch.object(sandbox, 'owner_wants_vr', return_value=None), \
                patch.object(sandbox, 'start_controller', side_effect=AssertionError('must not start')):
            with self.assertRaisesRegex(RuntimeError, 'not restored'):
                sandbox.acquire_turn('t', wait_s=0, say=lambda *a, **k: said.append(a[0]))
        self.assertIn('Orbiters test headset', said[0])


if __name__ == '__main__':
    unittest.main()
