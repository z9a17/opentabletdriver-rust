"""Offline test-folder preflight; no driver, hardware or child process is used."""
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("update_test_build", SCRIPTS / "update-test-build.py")
update = importlib.util.module_from_spec(spec)
spec.loader.exec_module(update)


@unittest.skipUnless(os.name == "nt", "Windows file/image sharing semantics")
class WritableTargetsTests(unittest.TestCase):
    def test_mapped_current_executable_rejects_before_install_and_releases_handles(self):
        with tempfile.TemporaryDirectory() as scratch:
            earlier = Path(scratch) / "earlier.txt"
            earlier.write_bytes(b"original")
            entered = False
            # Python's own running executable is already an image mapping. The
            # preflight only requests a handle; it never writes to that image.
            with self.assertRaises(PermissionError):
                update.preflight_targets([earlier, Path(sys.executable)])
                entered = True
                earlier.write_bytes(b"replaced")
            self.assertFalse(entered)
            self.assertEqual(earlier.read_bytes(), b"original")
            # MoveFileEx replacement fails if the earlier probe handle leaks.
            replacement = Path(scratch) / "replacement.txt"
            replacement.write_bytes(b"after preflight")
            os.replace(replacement, earlier)
            self.assertEqual(earlier.read_bytes(), b"after preflight")

    def test_ordinary_file_can_be_replaced_after_preflight(self):
        with tempfile.TemporaryDirectory() as scratch:
            destination = Path(scratch) / "payload.txt"
            staged = Path(scratch) / "staged.txt"
            destination.write_bytes(b"old")
            staged.write_bytes(b"new")
            update.preflight_targets([destination, Path(scratch) / "missing"])
            os.replace(staged, destination)
            self.assertEqual(destination.read_bytes(), b"new")


if __name__ == "__main__":
    unittest.main()
