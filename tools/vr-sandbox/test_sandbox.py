import hashlib
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
import sandbox

class RecoveryTests(unittest.TestCase):
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
