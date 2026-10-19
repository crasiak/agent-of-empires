"""OS-resource regression tests for the controlled supervisor, never native Pi."""
import errno
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("controlled_supervisor", Path(__file__).with_name("aoe-afk-observer-supervisor.py"))
supervisor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(supervisor)


class ResourceCleanupTests(unittest.TestCase):
    def test_second_actual_pipe_failure_closes_the_first_pair_and_root(self):
        roots, descriptors = [], []
        original_root, original_pipe = tempfile.mkdtemp, os.pipe
        with tempfile.TemporaryDirectory(prefix="aoe-supervisor-cleanup-test-") as base:
            def make_root(**kwargs):
                root = original_root(dir=base, **kwargs)
                roots.append(Path(root))
                return root

            def allocate():
                if descriptors:
                    raise OSError("controlled second allocation failure")
                pair = original_pipe()
                descriptors.extend(pair)
                return pair

            with mock.patch.object(supervisor.tempfile, "mkdtemp", side_effect=make_root), mock.patch.object(supervisor.os, "pipe", side_effect=allocate):
                result = supervisor.run("normal")
            self.assertEqual(result["error"], "setup_failed")
            self.assertTrue(result["process"]["cleanup_complete"])
            self.assertIsNone(result["completion"]["waitpid"])
            self.assertEqual(len(descriptors), 2)
            self.assertEqual(len(roots), 1)
            self.assertFalse(roots[0].exists())
            for fd in descriptors:
                with self.assertRaises(OSError) as error:
                    os.fstat(fd)
                self.assertEqual(error.exception.errno, errno.EBADF)

    def test_directory_setup_failure_removes_the_already_created_root(self):
        roots = []
        original_root, original_mkdir = tempfile.mkdtemp, Path.mkdir
        with tempfile.TemporaryDirectory(prefix="aoe-supervisor-directory-test-") as base:
            def make_root(**kwargs):
                root = original_root(dir=base, **kwargs)
                roots.append(Path(root))
                return root

            def mkdir(path, *args, **kwargs):
                if path.name == "xdg":
                    raise OSError("controlled directory allocation failure")
                return original_mkdir(path, *args, **kwargs)

            with mock.patch.object(supervisor.tempfile, "mkdtemp", side_effect=make_root), mock.patch.object(Path, "mkdir", new=mkdir):
                result = supervisor.run("normal")
            self.assertEqual(result["error"], "setup_failed")
            self.assertTrue(result["process"]["cleanup_complete"])
            self.assertFalse(roots[0].exists())

    def test_holder_spawn_failure_reaps_the_actual_main_child_before_removing_root(self):
        roots, children = [], []
        original_root, original_spawn = tempfile.mkdtemp, supervisor.subprocess.Popen
        with tempfile.TemporaryDirectory(prefix="aoe-supervisor-spawn-test-") as base:
            def make_root(**kwargs):
                root = original_root(dir=base, **kwargs)
                roots.append(Path(root))
                return root

            def spawn(*args, **kwargs):
                child = original_spawn(*args, **kwargs)
                children.append(child)
                return child

            with mock.patch.object(supervisor.tempfile, "mkdtemp", side_effect=make_root), mock.patch.object(supervisor.subprocess, "Popen", side_effect=spawn):
                result = supervisor.run("holder-spawn-failure")
            self.assertEqual(result["error"], "holder_spawn_failed")
            self.assertEqual(len(children), 1, "a real main child existed before the holder spawn failed")
            self.assertIsNotNone(children[0].returncode)
            self.assertIsNotNone(children[0].poll())
            self.assertTrue(result["process"]["main_reaped"])
            self.assertTrue(result["process"]["cleanup_complete"])
            self.assertFalse(roots[0].exists())


if __name__ == "__main__":
    unittest.main()
