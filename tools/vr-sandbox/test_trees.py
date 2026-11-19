"""SBX-9: the installed app's SteamVR driver folder and Steam's app manifest are put back only when this session can have
changed them, and a folder is never left half deleted."""
import shutil
import unittest
from pathlib import Path
from unittest.mock import patch

import sandbox_testbase  # first: every machine path into a temp folder
import sandbox


class Trees(unittest.TestCase):
    def setUp(self):
        shutil.rmtree(sandbox.BASE, ignore_errors=True)
        sandbox.ROOT.mkdir(parents=True)
        self.driver = sandbox.LOCAL / 'Orbiters/SteamVR'
        (self.driver / 'bin').mkdir(parents=True)
        (self.driver / 'bin/driver.dll').write_bytes(b'v1')
        self.manifest = sandbox.STEAM / 'config/steamapps.vrmanifest'
        self.manifest.parent.mkdir(parents=True)
        self.manifest.write_bytes(b'apps v1')
        self.bindings = sandbox.STEAM / 'config/vrappconfig'
        self.bindings.mkdir()
        (self.bindings / 'x.vrappconfig').write_text('{"autolaunch": true}')

    def session(self, test_app):
        j = {'session': 't', 'files': [], 'processes': [], 'restored': False, 'shared': True}
        if test_app:
            j['app_test_exe'] = 'test.exe'
        sandbox.backup(j, self.manifest)
        sandbox.backup(j, self.bindings, tree=True)
        sandbox.backup(j, self.driver, tree=True)
        for entry in j['files']:
            entry['policy'] = sandbox.restore_policy(entry)
        sandbox.atomic_json(sandbox.JOURNAL, j)
        # During the session: an update of the installed app brings driver v2, Steam rewrites its manifest, the sandbox
        # turns autolaunch off.
        (self.driver / 'bin/driver.dll').write_bytes(b'v2')
        self.manifest.write_bytes(b'apps v2')
        (self.bindings / 'x.vrappconfig').write_text('{"autolaunch": false}')
        return j

    def restore(self):
        with patch.object(sandbox, 'processes', return_value=[]):
            return sandbox.restore()

    def test_keep_app_session_without_a_test_program_keeps_the_apps_new_driver(self):
        self.session(test_app=False)
        result = self.restore()
        self.assertTrue(result['restored'])
        self.assertEqual((self.driver / 'bin/driver.dll').read_bytes(), b'v2')
        self.assertEqual(self.manifest.read_bytes(), b'apps v2')
        self.assertEqual((self.bindings / 'x.vrappconfig').read_text(), '{"autolaunch": true}', 'the sandbox\'s own change goes')
        self.assertIn('no test program', result['kept'][str(self.driver)])

    def test_a_test_program_session_puts_the_driver_folder_back(self):
        self.session(test_app=True)
        self.restore()
        self.assertEqual((self.driver / 'bin/driver.dll').read_bytes(), b'v1')
        self.assertEqual(self.manifest.read_bytes(), b'apps v2', 'the Steam client\'s manifest is never put back')

    def test_an_unchanged_folder_is_not_touched(self):
        j = self.session(test_app=True)
        (self.driver / 'bin/driver.dll').write_bytes(b'v1')
        with patch.object(sandbox.shutil, 'copytree', side_effect=AssertionError('copied')) as copy:
            entry = j['files'][2]
            self.assertEqual(sandbox.restore_tree(entry, self.driver, Path(entry['backup'])), 'unchanged')

    def test_a_failure_half_way_leaves_the_complete_old_or_new_folder(self):
        self.session(test_app=True)
        real = sandbox.os.replace
        def held(source, destination):    # a scanner holds a file: the current folder cannot be moved away
            if Path(source) == self.driver:
                raise PermissionError(5, 'Access is denied')
            return real(source, destination)
        with patch.object(sandbox.os, 'replace', side_effect=held):
            with self.assertRaises(PermissionError):
                self.restore()
        self.assertEqual((self.driver / 'bin/driver.dll').read_bytes(), b'v2', 'the complete current folder')
        self.assertFalse(self.driver.with_name('SteamVR.sandbox-restore').exists())
        self.assertFalse(sandbox.read_journal()['restored'])
        self.assertTrue(self.restore()['restored'], 'a later retry finishes')
        self.assertEqual((self.driver / 'bin/driver.dll').read_bytes(), b'v1')
        self.assertEqual(sorted(p.name for p in self.driver.parent.iterdir()), ['SteamVR'])

    def test_a_kill_between_the_two_renames_heals_on_retry(self):
        j = self.session(test_app=True)
        entry = j['files'][2]
        self.driver.rename(self.driver.with_name('SteamVR.sandbox-old'))     # killed after the first rename
        self.assertEqual(sandbox.restore_tree(entry, self.driver, Path(entry['backup'])), 'restored')
        self.assertEqual((self.driver / 'bin/driver.dll').read_bytes(), b'v1')
        self.assertFalse(self.driver.with_name('SteamVR.sandbox-old').exists())


if __name__ == '__main__':
    unittest.main()
