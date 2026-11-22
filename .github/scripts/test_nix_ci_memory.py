#!/usr/bin/env python3
"""Checks for the hosted Nix memory reservation and owned-file cleanup."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "nix_ci_memory", Path(__file__).with_name("nix-ci-memory.py")
)
memory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(memory)
GIB = 1024**3


class MemoryTests(unittest.TestCase):
    def test_reservation_accounts_for_existing_ram_swap_and_disk_headroom(self):
        for ram, swap, free, expected in (
            (7, 4, 30, 13), (16, 4, 20, 4), (32, 0, 8, 0),
            (7, 4, 20, None), (2, 0, 60, None),
        ):
            with self.subTest(ram=ram, swap=swap, free=free):
                if expected is None:
                    with self.assertRaises(RuntimeError):
                        memory.reservation(ram * GIB, swap * GIB, free * GIB)
                else:
                    self.assertEqual(memory.reservation(ram * GIB, swap * GIB, free * GIB), expected * GIB)

    def test_prepare_and_cleanup_only_change_the_new_owned_swap(self):
        for failure in (None, "mkswap", "swapon"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                state = root / "logs/memory.json"
                existing = root / "existing-swap"
                existing.write_text("unrelated existing swap")
                active = {str(existing)}
                commands = []
                allocations = []

                def run(command, **kwargs):
                    commands.append(command)
                    operation, path = command[3], command[-1]
                    if operation == "swapon":
                        active.add(path)
                    if operation == failure:
                        raise subprocess.CalledProcessError(1, command)
                    if operation == "swapoff":
                        active.remove(path)
                    return subprocess.CompletedProcess(command, 0)

                def allocate(fd, offset, size):
                    allocations.append(size)
                    os.ftruncate(fd, size)

                with patch.dict(os.environ, {"RUNNER_ENVIRONMENT": "github-hosted"}), \
                     patch.object(memory, "capacity", return_value=(7 * GIB, 4 * GIB, 30 * GIB)), \
                     patch.object(memory.os, "posix_fallocate", side_effect=allocate), \
                     patch.object(memory.subprocess, "run", side_effect=run), \
                     patch.object(memory, "active_swaps", side_effect=lambda: set(active)):
                    if failure:
                        with self.assertRaises(subprocess.CalledProcessError):
                            memory.prepare(state, root)
                    else:
                        memory.prepare(state, root)
                    data = json.loads(state.read_text())
                    owned = Path(data["swap"])
                    memory.cleanup(state, root)
                    memory.cleanup(state, root)
                self.assertEqual(allocations, [13 * GIB])
                self.assertFalse(owned.exists())
                self.assertTrue(json.loads(state.read_text())["cleaned"])
                self.assertEqual(active, {str(existing)})
                self.assertEqual(existing.read_text(), "unrelated existing swap")
                self.assertTrue(all(command[:3] == ["sudo", "-n", "--"] for command in commands))
                self.assertTrue(all(command[-1] == str(owned) for command in commands))
                self.assertNotIn("-a", [word for command in commands for word in command])

    def test_existing_capacity_and_nonhosted_refusal_do_not_allocate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(os.environ, {"RUNNER_ENVIRONMENT": "self-hosted"}), \
                 patch.object(memory, "capacity") as capacity:
                with self.assertRaises(RuntimeError):
                    memory.prepare(root / "refused.json", root)
                capacity.assert_not_called()
            with patch.dict(os.environ, {"RUNNER_ENVIRONMENT": "github-hosted"}), \
                 patch.object(memory, "capacity", return_value=(32 * GIB, 0, 8 * GIB)), \
                 patch.object(memory.os, "posix_fallocate") as allocate, \
                 patch.object(memory.subprocess, "run") as run:
                state = root / "enough.json"
                memory.prepare(state, root)
                memory.cleanup(state, root)
                allocate.assert_not_called()
                run.assert_not_called()
                self.assertTrue(json.loads(state.read_text())["cleaned"])

    def test_cleanup_refuses_replaced_symlinked_or_outside_reservation(self):
        for mutation in ("replacement", "symlink", "outside"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                parent = root / "aoe-nix-memory-fixture"
                parent.mkdir(mode=0o700)
                leaf = parent / "swap"
                leaf.write_bytes(b"fixture")
                leaf.chmod(0o600)
                state = root / "state.json"
                data = memory.identity(leaf)
                data.update({"swap": str(leaf), "size": 7, "cleaned": False})
                state.write_text(json.dumps(data))
                state.chmod(0o600)
                if mutation == "replacement":
                    leaf.rename(parent / "retained-original")
                    leaf.write_bytes(b"changed")
                    leaf.chmod(0o600)
                elif mutation == "symlink":
                    leaf.rename(parent / "retained-original")
                    leaf.symlink_to(parent / "retained-original")
                else:
                    data["swap"] = str(root / "outside")
                    state.write_text(json.dumps(data))
                with patch.object(memory.subprocess, "run") as run:
                    with self.assertRaises(RuntimeError):
                        memory.cleanup(state, root)
                    run.assert_not_called()
                self.assertTrue(leaf.exists())

    def test_swapoff_failure_retains_the_active_owned_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            parent = root / "aoe-nix-memory-fixture"
            parent.mkdir(mode=0o700)
            leaf = parent / "swap"
            leaf.write_bytes(b"fixture")
            leaf.chmod(0o600)
            state = root / "state.json"
            data = memory.identity(leaf)
            data.update({"swap": str(leaf), "size": 7, "cleaned": False})
            state.write_text(json.dumps(data))
            state.chmod(0o600)
            with patch.object(memory, "active_swaps", return_value={str(leaf)}), \
                 patch.object(memory.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "swapoff")):
                with self.assertRaises(subprocess.CalledProcessError):
                    memory.cleanup(state, root)
            self.assertTrue(leaf.exists())
            self.assertFalse(json.loads(state.read_text())["cleaned"])


if __name__ == "__main__":
    unittest.main()
