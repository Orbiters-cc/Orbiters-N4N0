"""SBX-1: a session that dies unrestored is retried until restored, every refusal is written down with its reason, and no
new session starts over it. No SteamVR: processes() is patched, every path lives under the temp test base."""
import json
import shutil
from pathlib import Path
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

    def test_an_empty_journal_falls_back_to_its_copy(self):
        # SBX-5: a power cut leaves session.json renamed but empty; session.prev.json still holds the session.
        sandbox.JOURNAL.write_bytes(b'')
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            self.assertTrue(sandbox.restore_orphan()['restored'])
        self.assertEqual(self.target.read_bytes(), b'original')
        self.assertTrue(sandbox.read_journal()['restored'], 'session.json written again')

    def test_an_unreadable_journal_is_written_down_not_a_crash(self):
        sandbox.JOURNAL.write_bytes(b'')
        sandbox.journal_prev().write_bytes(bytes(64))
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            result = sandbox.restore_orphan()
            self.assertFalse(result['restored'])
            self.assertIn('cannot be read', result['error'])
            self.assertIn('NOT restored', self.log())
            self.assertTrue(sandbox.MARKER.exists())
            with self.assertRaisesRegex(sandbox.JournalUnreadable, 'Nothing was changed'):
                sandbox.Session().start()
        self.assertEqual(self.target.read_bytes(), b'forced sandbox')

    def test_a_damaged_backup_is_never_copied_over_the_real_file(self):
        j = sandbox.read_journal()
        other = sandbox.ROOT.parent / 'openvrpaths.vrpath'
        other.write_bytes(b'paths original')
        sandbox.backup(j, other)
        sandbox.atomic_json(sandbox.JOURNAL, j)
        other.write_bytes(b'paths sandboxed')
        Path(j['files'][0]['backup']).write_bytes(b'')          # the settings backup came back empty after a power cut
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            result = sandbox.restore_orphan()
        self.assertFalse(result['restored'])
        self.assertIn('damaged backup', result['error']); self.assertIn('steamvr.vrsettings', result['error'])
        self.assertEqual(self.target.read_bytes(), b'forced sandbox', 'the damaged copy was not put over it')
        self.assertEqual(other.read_bytes(), b'paths original', 'the sound one was put back')
        self.assertFalse(sandbox.read_journal()['restored'])

    def test_the_watcher_keeps_trying_with_back_off_until_restored(self):
        answers = [RuntimeError('Restore refused while unowned VR processes run: vrserver.exe')] * 4 + [{'restored': True}]
        sleeps = []
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'restore', side_effect=answers):
            self.assertEqual(sandbox.watch({'pid': 1, 'create_time': 1.0}, 'dead1', sleep=sleeps.append), {'restored': True})
        self.assertEqual(sleeps, [5, 10, 20, 40])
        self.assertEqual(self.log().count('watcher: session dead1 NOT restored'), 1, 'the same reason is written once')

    def test_the_watcher_writes_again_when_the_reason_changes_or_after_half_an_hour(self):
        answers = [RuntimeError('a'), RuntimeError('a'), RuntimeError('b'), RuntimeError('b'), {'restored': True}]
        now = iter([0, 10, 20, 2000, 2010, 2020])
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'restore', side_effect=answers):
            sandbox.watch({'pid': 1, 'create_time': 1.0}, 'dead1', sleep=lambda _: None, clock=lambda: next(now))
        self.assertEqual(self.log().count('NOT restored'), 3)

    def test_a_damaged_backup_of_a_file_never_put_back_does_not_block(self):
        j = sandbox.read_journal()
        manifest = sandbox.ROOT.parent / 'steamapps.vrmanifest'
        manifest.write_bytes(b'apps')
        sandbox.backup(j, manifest); j['files'][-1]['policy'] = 'keep'
        sandbox.atomic_json(sandbox.JOURNAL, j)
        Path(j['files'][-1]['backup']).write_bytes(b'')
        with patch.object(sandbox, 'same_process', return_value=None), patch.object(sandbox, 'processes', return_value=[]):
            self.assertTrue(sandbox.restore_orphan()['restored'])

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


class SandboxStateIsNeverTheOriginal(unittest.TestCase):
    """SBX-15: with the journal lost, the live SteamVR files still say "sandbox": a new session refuses to back them up."""
    def setUp(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True)
        sandbox.ROOT.mkdir(parents=True)
        sandbox.SETTINGS.parent.mkdir(parents=True, exist_ok=True)
        sandbox.PATHS.parent.mkdir(parents=True, exist_ok=True)

    def start(self):
        with patch.object(sandbox, 'processes', return_value=[]), patch.object(sandbox, 'owner_wants_vr', return_value=None), \
                patch.object(sandbox.mmap, 'mmap', side_effect=AssertionError('no device memory')):
            sandbox.Session().start()

    def test_forced_driver_or_sandbox_driver_path_refuses_before_any_backup(self):
        clean_paths = json.dumps({'external_drivers': [], 'runtime': ['C:/SteamVR']})
        for settings, paths in ((json.dumps({'steamvr': {'forcedDriver': 'orbiters_sandbox'}}), clean_paths),
                                (json.dumps({'driver_orbiters_sandbox': {'enable': True}}), clean_paths),
                                (json.dumps({'steamvr': {}}), json.dumps({'external_drivers': [str(sandbox.ROOT / 'driver')]}))):
            sandbox.SETTINGS.write_text(settings); sandbox.PATHS.write_text(paths)
            with self.assertRaisesRegex(RuntimeError, 'sandbox state'):
                self.start()
            self.assertFalse((sandbox.ROOT / 'backup').exists())
            self.assertFalse(sandbox.LOCK.exists())
            self.assertEqual(sandbox.SETTINGS.read_text(), settings)

    def test_the_refusal_names_the_newest_clean_backup(self):
        import os, time
        old = sandbox.ROOT / 'backup-1'; old.mkdir()
        (old / '0').write_text(json.dumps({'external_drivers': []})); (old / '1').write_text(json.dumps({'steamvr': {}}))
        bad = sandbox.ROOT / 'backup-2'; bad.mkdir()
        (bad / '0').write_text(json.dumps({'external_drivers': [str(sandbox.ROOT / 'driver')]})); (bad / '1').write_text('{}')
        os.utime(old, (time.time() - 100, time.time() - 100))
        sandbox.SETTINGS.write_text(json.dumps({'steamvr': {'forcedDriver': 'orbiters_sandbox'}}))
        sandbox.PATHS.write_text('{}')
        with self.assertRaisesRegex(RuntimeError, 'backup-1'):
            self.start()

    def test_clean_files_pass(self):
        sandbox.SETTINGS.write_text(json.dumps({'steamvr': {'forcedDriver': ''}, 'driver_lighthouse': {}}))
        sandbox.PATHS.write_text(json.dumps({'external_drivers': ['D:/other/driver']}))
        self.assertEqual(sandbox.sandbox_leftovers(), [])


class DurableBackups(unittest.TestCase):
    def test_a_backup_is_verified_and_a_tree_gets_a_hash(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)
        folder = sandbox.ROOT.parent / 'tree'; shutil.rmtree(folder, ignore_errors=True); folder.mkdir(); (folder / 'a').write_bytes(b'1')
        j = {'files': []}
        sandbox.backup(j, folder, tree=True)
        self.assertEqual(j['files'][0]['tree_sha256'], sandbox.tree_sha256(folder))
        (Path(j['files'][0]['backup']) / 'a').write_bytes(b'2')
        self.assertIn('damaged', sandbox.backup_problem(j['files'][0]))
        single = sandbox.ROOT.parent / 'one'; single.write_bytes(b'x')
        with patch.object(sandbox, 'sha256_file', return_value='different'):
            with self.assertRaisesRegex(RuntimeError, 'differs from its original'):
                sandbox.backup(j, single)

    def test_journal_writes_are_flushed_before_the_rename(self):
        order = []
        real_fsync, real_replace = sandbox.os.fsync, sandbox.os.replace
        with patch.object(sandbox.os, 'fsync', side_effect=lambda fd: (order.append('fsync'), real_fsync(fd))), \
                patch.object(sandbox.os, 'replace', side_effect=lambda a, b: (order.append('replace'), real_replace(a, b))):
            sandbox.ROOT.mkdir(parents=True, exist_ok=True)
            sandbox.atomic_json(sandbox.JOURNAL, {'session': 'x', 'files': []})
        self.assertEqual(order, ['fsync', 'replace', 'fsync', 'replace'], 'journal, then its copy')
        self.assertEqual(json.loads(sandbox.journal_prev().read_text()), {'session': 'x', 'files': []})


class Pruning(unittest.TestCase):
    """SBX-16a: old backup folders are pruned; the newest five, the newest clean one and unrestored ones stay."""
    def test_prune(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)
        sandboxed = json.dumps({'external_drivers': [str(sandbox.ROOT / 'driver')]})
        for i in range(12):
            folder = sandbox.ROOT / f'backup-{1000 + i}'; folder.mkdir()
            (folder / '0').write_text('{}' if i == 2 else sandboxed); (folder / '1').write_text('{}')
        sandbox.atomic_json(sandbox.ROOT / 'backup-1004' / 'journal.json', {'session': 'u', 'restored': False})
        sandbox.atomic_json(sandbox.ROOT / 'backup-1005' / 'journal.json', {'session': 'r', 'restored': True})
        deleted = sandbox.prune_backups(keep=5)
        left = sorted(p.name for p in sandbox.ROOT.glob('backup-*'))
        self.assertEqual(left, ['backup-1002', 'backup-1004'] + [f'backup-{1000 + i}' for i in range(7, 12)])
        self.assertIn('backup-1005', deleted)

    def test_a_restore_marks_its_backup_folder(self):
        shutil.rmtree(sandbox.ROOT, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)
        target = sandbox.ROOT.parent / 'f'; target.write_text('a')
        j = {'session': 'm', 'files': [], 'processes': [], 'restored': False, 'shared': True, 'token': 'secret'}
        sandbox.backup(j, target); sandbox.atomic_json(sandbox.JOURNAL, j)
        sandbox.atomic_json(sandbox.ROOT / 'backup' / 'journal.json', {k: v for k, v in j.items() if k != 'token'})
        with patch.object(sandbox, 'processes', return_value=[]):
            sandbox.restore()
        copy = json.loads((sandbox.ROOT / 'backup' / 'journal.json').read_text())
        self.assertTrue(copy['restored']); self.assertNotIn('token', copy)
