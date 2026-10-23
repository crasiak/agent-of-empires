"""Injected orchestration and read-only capture tests. No AoE, tmux, Pi or provider executes."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import tomllib
import unittest

spec = importlib.util.spec_from_file_location("controller", Path(__file__).with_name("aoe-afk-native-controller.py"))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
a = c.a


def publish_nested(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    a.publish(path, value)


class FixedIO:
    """Fixed fake boundary events, not fabricated native qualification evidence."""
    def __init__(self, root, manifest, attempt, failure=None):
        self.root, self.manifest, self.attempt, self.failure = root, manifest, attempt, failure
        self.deadline = time.monotonic() + 10
        self.events = []
        self.b = {"instance_id": "session", "profile": "default", "native_id": "native", "launch_id": "launch"}

    def aoe(self, *args):
        app = Path(self.attempt["app"])
        if not (app / "profiles/default").is_dir():
            raise AssertionError("explicit private profile missing before managed command")
        policy = tomllib.loads((app / "config.toml").read_text())
        if policy != {"default_profile": "default", "systemd": {"enabled": False}, "updates": {"update_check_mode": "off"},
                      "session": {"restart_wake_message": "", "usage_tracking": False}}:
            raise AssertionError("private isolation policy missing before managed command")
        self.events.append(tuple(args))
        if args[0] == "migrate":
            return b""
        if args[0] == "add":
            publish_nested(Path(self.attempt["app"]) / "profiles/default/sessions.json", [{"id": "session", "tool": "pi", "command": ""}])
        if args[:2] == ("session", "start"):
            a.publish(self.root / "bootstrap.json", {"binding": self.b})
            a.publish(self.root / "ready.json", {"binding": self.b, "ready": True})
        if args[:4] == ("session", "afk", "batch", "preview"):
            preview = {"format": "aoe-afk-batch-preview", "version": 3, "operation_id": "operation", "profile": "default", "minutes": 1,
                       "shared_requests": None, "members": [{"session_id": "session", "exclusions": [], "grant": self.attempt["grant"], "usage_guard": self.attempt["usage_guard"]}]}
            preview["digest"] = a.digest(preview)
            a.publish(self.root / "preview.json", preview)
            if self.failure != "approval-timeout":
                approval = c.preview_approval(self.manifest, self.attempt, preview)
                if self.failure == "wrong-approval":
                    approval["preview_digest"] = "wrong"
                a.publish(self.root / "preview-approval.json", approval)
        if args[:4] == ("session", "afk", "batch", "apply"):
            self.assert_apply_admitted()
            if self.failure == "apply-fails":
                raise ValueError("fixed apply failure after admission")
            return b'{"operation_id":"operation","members":[]}'
        if args[:3] == ("session", "afk", "off"):
            publish_nested(Path(self.attempt["app"]) / "afk-authority-v3/profiles/default/snapshot.json",
                           {"sessions": {"session": {"policy": {"enabled": False}, "book": {"windows": [{"state": "ended"}]}}}})
        return b""

    def assert_apply_admitted(self):
        if not (self.root / "apply-admitted.json").exists():
            raise AssertionError("apply before durable admission")
        approval = a.read_json(self.root / "preview-approval.json")
        if a.read_json(self.root / "apply-admitted.json") != approval:
            raise AssertionError("apply admission differs")

    def terminal(self):
        self.events.append("terminal")
        return {"pane": "%0"}

    def marker(self, name, seconds):
        self.events.append(name)
        if self.failure == "approval-timeout" and name == "preview-approval.json":
            raise ValueError("fixed approval deadline")
        if self.failure == "not-armed" and name == "wait-armed.json":
            return {"armed": False}
        fixed = {"pending.json": {"calls": 2, "active": True}, "wait-armed.json": {"armed": True},
                 "run-completed.json": {"completed": True}, "off-seen.json": {"off": True}, "process.json": {"waitpid": 0}}
        return fixed[name] if name in fixed else a.read_json(self.root / name)

    def capture(self, attempt, preview):
        self.events.append("capture")
        return {"fixture": "bounded capture callback"}

    def endpoint(self, captured):
        self.events.append("endpoint")
        return {"endpoint_status": "inconclusive" if self.failure == "endpoint" else "qualified"}

    def arm(self):
        self.events.append("arm")
        if not (self.root / "capture.json").exists():
            raise AssertionError("arm before capture")

    def control(self, command):
        self.events.append(command)
        if command == "cancel" and "wait-armed.json" not in self.events:
            raise AssertionError("cancellation before arm acknowledgement")


class ControllerTests(unittest.TestCase):
    def test_ordered_one_use_flow_and_failures_do_not_reapply(self):
        for failure in [None, "approval-timeout", "wrong-approval", "apply-fails", "not-armed", "endpoint"]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                prepared = Path(directory).resolve()
                root = prepared / "baseline"
                manifest = {"root": str(prepared), "experiment": a.proposal(), "config": {"app_name": "agent-of-empires-dev"},
                            "tools": {name: {"path": f"/fixed/{name}"} for name in ["tmux", "fd", "rg"]},
                            "initial_seconds": 30, "preview_approval_seconds": 30, "capture_attempts": 2, "capture_bytes": 1000000}
                attempt = c.initialize(root, manifest, "baseline", 1)
                config_path = Path(attempt["app"]) / "config.toml"
                self.assertEqual(config_path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(attempt["usage_guard"]["usd"]["limit_nano"], 100)
                io = FixedIO(root, manifest, attempt, failure)
                if failure:
                    with self.assertRaises(ValueError):
                        c.orchestrate(io, manifest, attempt)
                else:
                    self.assertEqual(c.orchestrate(io, manifest, attempt)["controller"], "completed")
                    self.assertLess(io.events.index("capture"), io.events.index("arm"))
                    self.assertLess(io.events.index("wait-armed.json"), io.events.index("cancel"))
                    self.assertLess(io.events.index(("session", "afk", "off", "session")), io.events.index("shutdown"))
                    self.assertEqual(a.read_json(root / "apply-admitted.json"), a.read_json(root / "preview-approval-required.json"))
                applies = [event for event in io.events if isinstance(event, tuple) and event[:4] == ("session", "afk", "batch", "apply")]
                self.assertLessEqual(len(applies), 1)
                if failure in ("approval-timeout", "wrong-approval"):
                    self.assertEqual(applies, [])
                    self.assertFalse((root / "apply-admitted.json").exists())
                if failure in ("not-armed", "endpoint"):
                    self.assertNotIn("cancel", io.events)
                if failure:
                    self.assertTrue((root / "controller-error.json").exists())

    def test_capture_is_bounded_read_only_and_refuses_linked_native_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            work, sessions, app = root / "work", root / "sessions", root / "app"
            work.mkdir(mode=0o700); sessions.mkdir(mode=0o700)
            profile = app / "afk-authority-v3/profiles/default"
            transport = profile / "transport/session"
            transport.mkdir(mode=0o700, parents=True)
            for parent in [app, app / "afk-authority-v3", app / "afk-authority-v3/profiles", profile, profile / "transport"]:
                parent.chmod(0o700)
            b = {"instance_id": "session", "profile": "default", "native_id": "native", "launch_id": "launch"}
            native = sessions / "native.jsonl"
            native.write_bytes(b'{"type":"session","id":"native"}\n')
            native.chmod(0o600)
            a.publish(root / "bootstrap.json", {"binding": b, "app_dir": str(app)})
            a.publish(root / "endpoint-native.json", {"file": str(native)})
            a.publish(root / "ready.json", {"binding": b})
            a.publish(root / "apply-result.json", {})
            a.publish(root / "apply-admitted.json", {})
            a.publish(root / "attempt.json", {})
            a.publish(profile / "snapshot.json", {"version": 3})
            (profile / "writer.lock").write_bytes(b"")
            (profile / "writer.lock").chmod(0o600)
            publish_nested(app / "profiles/default/sessions.json", [])
            a.publish(transport / "usage-reporter.json", {"fixed": "reporter"})
            a.publish(transport / "usage-lease.json", {"fixed": "lease"})
            before = {str(p): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            got = c.capture(root, {"app": str(app)}, {"operation_id": "operation"}, "digest", clock=lambda: {"boot": "fixed", "millis": 1})
            self.assertTrue(got["stable"])
            self.assertTrue(got["pending_absent"])
            self.assertTrue(got["scratch_absent"])
            self.assertEqual(got["native_rows"], [{"type": "session", "id": "native"}])
            after = {str(p): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            self.assertEqual(before, after)
            native.unlink()
            native.symlink_to(root / "ready.json")
            with self.assertRaises(OSError):
                c.capture(root, {"app": str(app)}, {"operation_id": "operation"}, "digest", clock=lambda: {})


if __name__ == "__main__":
    unittest.main()
