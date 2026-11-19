"""SBX-4: VRChat's saved window size (registry) is put back after a session that launched VRChat. Under the test base the
registry is an in-memory fake (sandbox.TEST_REGISTRY): no test touches the owner's registry."""
import shutil
import unittest
from unittest.mock import patch

import sandbox_testbase  # first: every machine path into a temp folder
import sandbox

DWORD = 4
OWNER = {'Screenmanager Resolution Width_h182942802': (DWORD, 2560), 'Screenmanager Resolution Height_h2627697771': (DWORD, 1440),
         'Screenmanager Fullscreen mode_h3630240806': (DWORD, 1), 'UnityGraphicsQuality_h1669003810': (DWORD, 3)}


class VRChatWindow(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(sandbox.TEST_REGISTRY, 'tests run on the fake registry')
        sandbox.TEST_REGISTRY.clear(); sandbox.TEST_REGISTRY.update(OWNER)
        shutil.rmtree(sandbox.ROOT, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)

    def launch_and_quit(self):
        s = sandbox.Session()
        s.j = {'session': 'w', 'files': [], 'processes': [], 'restored': False, 'shared': True}
        with patch.object(sandbox.vrchat_life, 'wait_idle', return_value=0), patch.object(sandbox.vrchat_life, 'Sampler'), \
                patch.object(sandbox.subprocess, 'Popen'), patch.object(sandbox.Session, 'record'), \
                patch.object(sandbox.Session, 'vrchat', return_value={'pid': 5, 'create_time': 2e9}):
            s.launch('vrchat://launch?ref=orbiters.cc&id=x', 0)
        # VRChat quits and Unity saves the sandbox's window: 1280x720, windowed, and a value it did not have before.
        sandbox.TEST_REGISTRY.update({'Screenmanager Resolution Width_h182942802': (DWORD, 1280),
                                      'Screenmanager Resolution Height_h2627697771': (DWORD, 720),
                                      'Screenmanager Fullscreen mode_h3630240806': (DWORD, 3),
                                      'Screenmanager Window Position X_h1': (DWORD, 10),
                                      'UnityGraphicsQuality_h1669003810': (DWORD, 5)})
        return s.j

    def test_the_owners_window_comes_back(self):
        j = self.launch_and_quit()
        with patch.object(sandbox, 'processes', return_value=[]):
            result = sandbox.restore_contents(j)
        self.assertEqual(result['vrchat_window'], 'put back')
        expected = dict(OWNER); expected['UnityGraphicsQuality_h1669003810'] = (DWORD, 5)
        self.assertEqual(dict(sandbox.TEST_REGISTRY), expected, 'only Screenmanager values, and only back to their start')

    def test_the_snapshot_is_taken_before_the_launch(self):
        j = self.launch_and_quit()
        first = j['vrchat_window']
        self.assertIn(['Screenmanager Resolution Width_h182942802', DWORD, 2560], first)

    def test_a_session_without_vrchat_leaves_it(self):
        with patch.object(sandbox, 'processes', return_value=[]):
            result = sandbox.restore_contents({'session': 'n', 'files': [], 'processes': [], 'restored': False, 'shared': True})
        self.assertEqual(result['vrchat_window'], 'no VRChat launched by the session')


if __name__ == '__main__':
    unittest.main()


class InstalledAppShutdownSave(unittest.TestCase):
    """SBX-3: the installed app saves its pending settings when it quits; the backup holds that save."""
    def test_settings_are_backed_up_after_the_apps_quit(self):
        from pathlib import Path
        from unittest.mock import Mock
        shutil.rmtree(sandbox.BASE, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)
        install = sandbox.LOCAL / 'Programs/Orbiters - N4N0/orbiters-desktop.exe'
        install.parent.mkdir(parents=True); install.write_bytes(b'')
        settings = sandbox.ROAMING / 'Orbiters/Desktop/settings.json'
        settings.parent.mkdir(parents=True); settings.write_text('A')
        app = Mock(); app.info = {'name': install.name}; app.exe.return_value = str(install); app.cmdline.return_value = [str(install)]
        def quit(args, **kwargs):
            self.assertEqual(args, [str(install), '--quit'])
            settings.write_text('B')     # its shutdown save
        s = sandbox.Session()
        s.j = {'session': 'q', 'files': [], 'processes': [], 'restored': False, 'shared': True}
        with patch.object(sandbox.psutil, 'process_iter', return_value=[app]), patch.object(sandbox.subprocess, 'run', side_effect=quit), \
                patch.object(sandbox.psutil, 'wait_procs', return_value=([app], [])):
            s.stop_installed_app()
        saved = next(e for e in s.j['files'] if e['path'] == str(settings))
        self.assertEqual(Path(saved['backup']).read_text(), 'B')
        self.assertTrue(s.j['installed_app_stopped'])

    def test_an_app_that_does_not_quit_leaves_the_stop_to_be_done_again(self):
        from unittest.mock import Mock
        shutil.rmtree(sandbox.BASE, ignore_errors=True); sandbox.ROOT.mkdir(parents=True)
        s = sandbox.Session()
        s.j = {'session': 'q', 'files': [], 'processes': [], 'restored': False, 'shared': True}
        with patch.object(sandbox.psutil, 'process_iter', return_value=[]), patch.object(sandbox.psutil, 'wait_procs', return_value=([], [Mock()])):
            with self.assertRaisesRegex(RuntimeError, 'did not finish quitting'):
                s.stop_installed_app()
        self.assertFalse(s.j.get('installed_app_stopped'))
        self.assertEqual(s.j['files'], [])
