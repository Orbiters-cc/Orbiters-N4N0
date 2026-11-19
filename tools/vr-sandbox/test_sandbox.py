import hashlib
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import sandbox_testbase  # first: every machine path into a temp folder
import sandbox

class JournalTests(unittest.TestCase):
    def test_temporary_windows_reader_does_not_abort_the_session(self):
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'journal.json';path.write_text('{"session":"old"}')
            replace=sandbox.os.replace
            denied=PermissionError('journal reader holds Windows delete sharing');denied.winerror=5
            attempts=[]
            def sharing_then_replace(source,destination):
                attempts.append(1)
                self.assertEqual(json.loads(path.read_text()),{'session':'old'})
                if len(attempts)<3:raise denied
                replace(source,destination)
            with patch.object(sandbox.os,'replace',side_effect=sharing_then_replace),patch.object(sandbox.time,'sleep'):
                sandbox.atomic_json(path,{'session':'new'})
            self.assertEqual(json.loads(path.read_text()),{'session':'new'})
            self.assertEqual(len(attempts),3)

    def test_persistent_windows_sharing_failure_preserves_previous_journal(self):
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'journal.json';original=b'{"session":"old"}';path.write_bytes(original)
            denied=PermissionError('persistent sharing violation');denied.winerror=32
            with patch.object(sandbox.os,'replace',side_effect=denied) as replace,patch.object(sandbox.time,'sleep'):
                with self.assertRaises(PermissionError):sandbox.atomic_json(path,{'session':'new'})
            self.assertEqual(replace.call_count,20)
            self.assertEqual(path.read_bytes(),original)
            self.assertFalse(path.with_suffix('.tmp').exists())

class RotationTests(unittest.TestCase):
    def test_half_turn_uses_continuous_representation_without_changing_orientation(self):
        import math
        for raw in [[0,1,0,0],[0,0,.9238795325,.3826834324],[0,0,-.9238795325,-.3826834324]]:
            q=sandbox.unit_rotation(raw);n=math.sqrt(sum(x*x for x in raw));expected=[x/n for x in raw]
            self.assertNotEqual(q[0],0)
            self.assertLess(sum((a-b)**2 for a,b in zip(q,expected)),1e-28)
            self.assertAlmostEqual(sum(x*x for x in q),1,places=14)
        with self.assertRaisesRegex(ValueError,'zero quaternion'):sandbox.unit_rotation([0,0,0,0])

class ProcessTests(unittest.TestCase):
    def test_dead_processes_do_not_refuse_new_sessions(self):
        from unittest.mock import Mock
        for threads,handles in [(0,4),(2,0)]:
            p=Mock();p.num_threads.return_value=threads;p.num_handles.return_value=handles
            self.assertTrue(sandbox.process_dead(p))
            with patch.object(sandbox,'process_names',return_value=[(123,'vrwebhelper.exe')]),patch.object(sandbox.psutil,'Process',return_value=p):
                self.assertEqual(sandbox.processes(),[])
        p=Mock();p.status.side_effect=sandbox.psutil.NoSuchProcess(123)
        self.assertTrue(sandbox.process_dead(p))

    def test_only_runtime_names_are_queried_for_creation_identity(self):
        with patch.object(sandbox,'process_exited',return_value=False),patch.object(sandbox,'process_names',return_value=[(1,'unrelated.exe'),(2,'VRChat.exe'),(3,'vrserver.exe')]),patch.object(sandbox.psutil,'Process') as process:
            process.return_value.create_time.return_value=123.5
            self.assertEqual(sandbox.processes(),[{'pid':2,'name':'VRChat.exe','create_time':123.5},{'pid':3,'name':'vrserver.exe','create_time':123.5}])
            self.assertEqual([call.args[0] for call in process.call_args_list],[2,3])

    def test_failed_snapshot_is_an_error_not_a_claim_of_no_vr(self):
        with patch.object(sandbox.kernel,'CreateToolhelp32Snapshot',return_value=sandbox.C.c_void_p(-1).value):
            with self.assertRaises(OSError):sandbox.processes()

class RecoveryTests(unittest.TestCase):
    def test_exiting_owned_helper_warns_and_restores_files(self):
        from unittest.mock import Mock
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);target=root/'settings';target.write_bytes(b'original')
            info={'pid':123,'create_time':1,'name':'vrwebhelper.exe'}
            j={'session':'exiting','files':[],'processes':[info],'restored':False}
            p=Mock();p.pid=123;p.create_time.return_value=1
            with patch.multiple(sandbox,ROOT=root,LOCK=root/'lock',JOURNAL=root/'journal'),patch.object(sandbox,'processes',return_value=[info]),patch.object(sandbox,'same_process',return_value=p),patch.object(sandbox.psutil,'wait_procs',return_value=([],[p])),patch.object(sandbox,'process_dead',return_value=False),patch.object(sandbox,'terminate_dead_helper',return_value=True):
                sandbox.backup(j,target);target.write_bytes(b'changed')
                result=sandbox.restore_contents(j)
                self.assertEqual(result['leftover_dead_pids'],[123]);self.assertEqual(target.read_bytes(),b'original')
                self.assertEqual(json.loads(sandbox.JOURNAL.read_text())['dead_processes'],[info])

    def test_stale_watchdog_cannot_stop_a_new_session(self):
        with tempfile.TemporaryDirectory() as folder:
            journal=Path(folder)/'journal';journal.write_text(json.dumps({'session':'new','restored':False}))
            with patch.object(sandbox,'JOURNAL',journal),patch.object(sandbox,'remote') as remote:
                self.assertEqual(sandbox.restore(expected_session='old')['reason'],'session changed')
                remote.assert_not_called()

    def test_watchdog_and_manual_restore_copy_configuration_once(self):
        import time
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);target=root/'settings';target.write_bytes(b'original')
            j={'session':'concurrent','files':[],'processes':[],'restored':False}
            with patch.multiple(sandbox,ROOT=root,LOCK=root/'lock',JOURNAL=root/'journal'),patch.object(sandbox,'processes',return_value=[]):
                sandbox.backup(j,target);target.write_bytes(b'changed');sandbox.atomic_json(sandbox.JOURNAL,j)
                entered=threading.Event();release=threading.Event();original=sandbox.shutil.copy2;calls=[];errors=[];answers=[]
                def copy(source,destination,*args,**kwargs):
                    calls.append(destination);entered.set();release.wait(2)
                    return original(source,destination,*args,**kwargs)
                def recover():
                    try:answers.append(sandbox.restore())
                    except Exception as error:errors.append(error)
                with patch.object(sandbox.shutil,'copy2',side_effect=copy):
                    first=threading.Thread(target=recover);second=threading.Thread(target=recover);first.start();self.assertTrue(entered.wait(2));second.start();time.sleep(.05);release.set();first.join(3);second.join(3)
                self.assertFalse(first.is_alive() or second.is_alive());self.assertFalse(errors);self.assertEqual(len(calls),1);self.assertEqual(len(answers),2);self.assertEqual(target.read_bytes(),b'original')

    def test_refusing_an_existing_vr_session_never_opens_device_ipc(self):
        with patch.object(sandbox,'processes',return_value=[{'pid':1,'name':'VRChat.exe'}]),patch.object(sandbox.mmap,'mmap') as mapping:
            session=sandbox.Session()
            with self.assertRaisesRegex(RuntimeError,'Person is using VR'):session.start()
            mapping.assert_not_called()

    def test_restore_bytes_absence_and_tree_and_idempotence(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); original=root/'settings'; original.write_bytes(b'original\r\n\x00')
            absent=root/'absent'; tree=root/'bindings';tree.mkdir();(tree/'a').write_bytes(b'binding')
            j={'session':'test','files':[],'processes':[],'restored':False}
            with patch.multiple(sandbox,ROOT=root,LOCK=root/'lock',JOURNAL=root/'journal'),patch.object(sandbox,'processes',return_value=[]):
                for p,t in [(original,False),(absent,False),(tree,True)]:sandbox.backup(j,p,t)
                sandbox.atomic_json(sandbox.JOURNAL,j);sandbox.LOCK.write_text(json.dumps({'session':'test'}))
                original.write_text('changed');absent.write_text('new');(tree/'extra').write_text('new')
                self.assertTrue(sandbox.restore()['restored'])
                self.assertEqual(original.read_bytes(),b'original\r\n\x00');self.assertFalse(absent.exists())
                self.assertEqual(list(p.name for p in tree.iterdir()),['a']);self.assertFalse(sandbox.LOCK.exists())
                self.assertTrue(sandbox.restore()['already_restored'])

    def test_foreign_process_prevents_writes_and_keeps_recovery(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);journal=root/'journal';journal.write_text(json.dumps({'session':'x','files':[],'processes':[],'restored':False}))
            with patch.multiple(sandbox,JOURNAL=journal),patch.object(sandbox,'processes',return_value=[{'pid':123,'create_time':1,'name':'VRChat.exe'}]):
                with self.assertRaisesRegex(RuntimeError,'unowned VR processes'):sandbox.restore()
                self.assertFalse(json.loads(journal.read_text())['restored'])

class InputTests(unittest.TestCase):
    def test_camera_modes_reject_unknown_values_before_sending_osc(self):
        s=self.session()
        with patch.object(sandbox.socket,'socket') as socket:
            for mode in [-1,7,True,1.5]:
                with self.assertRaisesRegex(ValueError,'mode'):s.call({'action':'camera_mode','mode':mode})
            socket.assert_not_called()

    def session(self):
        s=object.__new__(sandbox.Session)
        s.shared=sandbox.Shared();s.guard=threading.Lock();s.stop=threading.Event();s.mutex=123;s.j={}
        return s

    def test_trigger_press_releases_analog_click_and_touch(self):
        s=self.session();observed=[]
        with patch.object(sandbox.kernel,'WaitForSingleObject',return_value=0),patch.object(sandbox.kernel,'ReleaseMutex'),patch.object(s.stop,'wait',side_effect=lambda _:observed.append(list(s.shared.devices[2].inputs))):
            s.call({'action':'input','device':'right','input':'trigger','value':1,'duration_s':.12})
        self.assertEqual([observed[0][i] for i in [0,4,19]],[1,1,1])
        self.assertEqual([s.shared.devices[2].inputs[i] for i in [0,4,19]],[0,0,0])

    def test_bad_duration_does_not_leave_any_input_down(self):
        s=self.session()
        with self.assertRaisesRegex(ValueError,'duration'):
            s.call({'action':'input','device':'right','input':'trigger','value':1,'duration_s':99})
        self.assertEqual(list(s.shared.devices[2].inputs),[0]*20)

    def test_flick_changes_both_axes_together_and_returns_to_center(self):
        s=self.session();observed=[]
        with patch.object(sandbox.kernel,'WaitForSingleObject',return_value=0),patch.object(sandbox.kernel,'ReleaseMutex'),patch.object(s.stop,'wait',side_effect=lambda _:observed.append(list(s.shared.devices[2].inputs)[2:4])):
            s.call({'action':'flick','device':'right','stick':[-.8,.5]})
        self.assertAlmostEqual(observed[0][0],-.8);self.assertEqual(observed[0][1],.5)
        self.assertEqual(observed[1],[0,0])

if __name__=='__main__':unittest.main()


class BridgeSessionsBeside(unittest.TestCase):
    """A desktop VRChat of Orbiters' bridge in a profile of its own (2026-11-15) coexists with the shared session."""

    def cmdline(self, args):
        from unittest.mock import Mock
        process = Mock()
        process.cmdline.return_value = args
        return patch.object(sandbox.vrchat_life.psutil, 'Process', return_value=process)

    def test_a_bridge_session_is_told_apart_by_its_own_profile_and_osc(self):
        bridge = ['C:/VRChat/VRChat.exe', '--no-vr', '--profile=1', '--osc=9010:127.0.0.1:9011', '-screen-fullscreen', '0']
        with self.cmdline(bridge):
            self.assertTrue(sandbox.vrchat_life.bridge_session(1))
        for other in (['C:/VRChat/VRChat.exe', 'vrchat://launch?id=x', '-screen-fullscreen', '0'],          # the sandbox's or the owner's VR
                      ['C:/VRChat/VRChat.exe', '--no-vr', '--profile=0', '--osc=9010:127.0.0.1:9011'],    # the owner's own profile
                      ['C:/VRChat/VRChat.exe', '--no-vr', '--profile=2'],                                 # no ports of its own
                      ['C:/VRChat/VRChat.exe', '--profile=1', '--osc=9010:127.0.0.1:9011']):              # in VR
            with self.cmdline(other):
                self.assertFalse(sandbox.vrchat_life.bridge_session(1), other)

    def test_a_bridge_session_neither_refuses_the_sandbox_nor_is_recorded_as_its_own(self):
        with patch.object(sandbox, 'process_exited', return_value=False), \
                patch.object(sandbox, 'process_names', return_value=[(2, 'VRChat.exe'), (3, 'vrserver.exe')]), \
                patch.object(sandbox.vrchat_life, 'bridge_session', side_effect=lambda pid, *_: pid == 2), \
                patch.object(sandbox.psutil, 'Process') as process:
            process.return_value.create_time.return_value = 1.0
            self.assertEqual([p['pid'] for p in sandbox.processes()], [3])

    def test_a_launch_beside_a_bridge_session_waits_for_nothing_but_a_launcher(self):
        running = {'state': 'RUNNING'}
        beside = [{'name': 'VRChat.exe', 'pid': 2}]
        eac = {'name': 'EasyAntiCheat_EOS.exe', 'pid': 9}
        snap = lambda procs: {'services': {'EasyAntiCheat_EOS': running, 'EasyAntiCheat_EOSSys': running}, 'processes': procs, 'beside': beside}
        self.assertTrue(sandbox.vrchat_life.idle(snap([eac])))
        self.assertFalse(sandbox.vrchat_life.idle(snap([eac, {'name': 'start_protected_game.exe', 'pid': 7}])), 'a launch under way')
        self.assertFalse(sandbox.vrchat_life.idle(snap([eac, {'name': 'VRChat.exe', 'pid': 8}])), 'the sandbox\'s own VRChat still runs')
        stopped = {'services': {'EasyAntiCheat_EOS': {'state': 'STOPPED'}, 'EasyAntiCheat_EOSSys': {'state': 'RUNNING'}}, 'processes': [], 'beside': []}
        self.assertFalse(sandbox.vrchat_life.idle(stopped), 'alone, the 2026-11-13 rule stands')

    def launched_with(self, profile):
        """The command line `Session.launch` gives launch.exe, with EAC idle and VRChat up at once."""
        from unittest import mock
        s = sandbox.Session()
        s.j = {'processes': []}
        with mock.patch.object(sandbox.vrchat_life, 'wait_idle', return_value=0),              mock.patch.object(sandbox.vrchat_life, 'Sampler'),              mock.patch.object(sandbox, 'atomic_json'),              mock.patch.object(sandbox.subprocess, 'Popen') as popen,              mock.patch.object(sandbox.Session, 'record'),              mock.patch.object(sandbox.Session, 'vrchat', return_value={'pid': 5, 'create_time': 2e9}):
            s.launch('vrchat://launch?ref=orbiters.cc&id=x', profile)
        return popen.call_args[0][0][1:], s

    def test_the_vr_launch_names_the_default_profile(self):
        args, s = self.launched_with(0)
        self.assertEqual(args[:2], ['vrchat://launch?ref=orbiters.cc&id=x', '--profile=0'])
        self.assertFalse(any(a.startswith('--osc') for a in args), "the owner's VRChat keeps VRChat's own OSC ports")
        self.assertNotIn('--no-vr', args)
        self.assertEqual(s.osc_in(), 9000)

    def test_a_bridge_sessions_vr_launch_has_its_profile_and_its_osc_ports(self):
        args, s = self.launched_with(1)
        self.assertIn('--profile=1', args)
        self.assertIn('--osc=9010:127.0.0.1:9011', args, 'the ports the bridge gives profile 1 (sessions.rs)')
        self.assertNotIn('--no-vr', args, 'VR mode: controllers, not the keyboard and mouse')
        self.assertEqual(s.osc_in(), 9010, 'neutral and camera_mode talk to that VRChat')
        self.assertFalse(sandbox.vrchat_life.bridge_session(0, type('P', (), {'cmdline': lambda self: ['VRChat.exe', *args]})()),
                         'a VR VRChat of the sandbox is never taken for a desktop bridge session')

    def test_keep_app_leaves_the_installed_app_running(self):
        source = Path(sandbox.__file__).read_text(encoding='utf-8')
        self.assertIn("if not keep_app:self.stop_installed_app()", source)
        self.assertIn("'--keep-app'", source)


class PointingTheLaser(unittest.TestCase):
    """`point`: the controller is placed so that VRChat's laser (laser_deg below its -Z) goes through the picked pixel."""
    def view(self, yaw_deg=0.0):
        import math
        c, s = math.cos(math.radians(yaw_deg)), math.sin(math.radians(yaw_deg))
        head = [c, 0, s, 0.2, 0, 1, 0, 1.65, -s, 0, c, -0.1]
        eye = [1, 0, 0, -0.032, 0, 1, 0, 0, 0, 0, 1, 0.015]
        return {'head': head, 'eyes': [{'eye_to_head': eye, 'projection': [-1.0, 1.0, -1.2, 1.2]}]}

    def laser(self, view, pixel, laser_deg=64.5):
        import geometry, math
        position, q = geometry.pointing_pose(view, pixel, [1000, 1000], {'origin': [0, 0, 0], 'rotate_xyz': [-laser_deg, 0, 0]})
        a = math.radians(laser_deg)
        return position, geometry.rotate(q, [0, -math.sin(a), -math.cos(a)])

    def test_the_laser_goes_through_the_picked_pixel(self):
        import math
        for yaw in (0.0, 35.0, -120.0):
            v = self.view(yaw)
            for pixel, ray in (([500, 500], [0, 0, -1]), ([750, 250], [0.5, 0.6, -1]), ([100, 900], [-0.8, -0.96, -1])):
                _, laser = self.laser(v, pixel)
                c, s = math.cos(math.radians(yaw)), math.sin(math.radians(yaw))
                world = [c * ray[0] + s * ray[2], ray[1], -s * ray[0] + c * ray[2]]
                n = math.sqrt(sum(x * x for x in world))
                for got, want in zip(laser, [x / n for x in world]):
                    self.assertAlmostEqual(got, want, places=6, msg=(yaw, pixel))

    def test_the_controller_sits_just_in_front_of_the_eye(self):
        position, _ = self.laser(self.view(0.0), [500, 500])
        self.assertAlmostEqual(position[0], 0.2 - 0.032, places=6)
        self.assertAlmostEqual(position[1], 1.65, places=6)
        self.assertAlmostEqual(position[2], -0.1 + 0.015 - 0.05, places=6)


class TestBase(unittest.TestCase):
    """No test reaches the owner's machine: every path sandbox.py changes lives under the temp test base."""
    def test_every_machine_path_is_inside_the_test_base(self):
        base = Path(sandbox_testbase.BASE).resolve()
        for path in (sandbox.ROOT, sandbox.LOCK, sandbox.JOURNAL, sandbox.STEAM, sandbox.PATHS, sandbox.SETTINGS, sandbox.HOME,
                     sandbox.LOCAL, sandbox.ROAMING):
            self.assertTrue(Path(path).resolve().is_relative_to(base), path)

    def test_a_test_base_outside_the_temp_directory_is_refused(self):
        import os
        with patch.dict(os.environ, {'ORBITERS_VR_SANDBOX_TEST_BASE': str(Path(sandbox.__file__).parent)}):
            with self.assertRaises(SystemExit):sandbox.machine_base()
