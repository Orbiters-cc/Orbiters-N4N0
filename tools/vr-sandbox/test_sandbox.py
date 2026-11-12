import hashlib
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
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
        with patch.object(sandbox,'process_names',return_value=[(1,'unrelated.exe'),(2,'VRChat.exe'),(3,'vrserver.exe')]),patch.object(sandbox.psutil,'Process') as process:
            process.return_value.create_time.return_value=123.5
            self.assertEqual(sandbox.processes(),[{'pid':2,'name':'VRChat.exe','create_time':123.5},{'pid':3,'name':'vrserver.exe','create_time':123.5}])
            self.assertEqual([call.args[0] for call in process.call_args_list],[2,3])

    def test_failed_snapshot_is_an_error_not_a_claim_of_no_vr(self):
        with patch.object(sandbox.kernel,'CreateToolhelp32Snapshot',return_value=sandbox.C.c_void_p(-1).value):
            with self.assertRaises(OSError):sandbox.processes()

class RecoveryTests(unittest.TestCase):
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
        s.shared=sandbox.Shared();s.guard=threading.Lock();s.stop=threading.Event();s.mutex=123
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
