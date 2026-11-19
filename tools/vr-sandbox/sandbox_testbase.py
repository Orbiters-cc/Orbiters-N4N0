"""Imported first by every sandbox test: moves all of sandbox.py's machine paths (home, AppData, Steam, the sandbox root)
into a fresh folder under the temp directory, so no test can read or change the owner's SteamVR files or the real
sandbox state in C:/Users/<owner>/.orbiters-build/vr-sandbox. sandbox.py refuses a test base outside the temp directory."""
import atexit
import os
import shutil
import tempfile

BASE = tempfile.mkdtemp(prefix='vr-sandbox-test-')
os.environ['ORBITERS_VR_SANDBOX_TEST_BASE'] = BASE
atexit.register(shutil.rmtree, BASE, True)
