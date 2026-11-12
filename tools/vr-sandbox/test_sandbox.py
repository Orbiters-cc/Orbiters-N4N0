import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import sandbox

class RecoveryTests(unittest.TestCase):
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

if __name__=='__main__':unittest.main()
