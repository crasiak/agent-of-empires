import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "aoe_batch", Path(__file__).parents[1] / "aoe-batch.py"
)
batch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(batch)
SID = "0123456789abcdef"
NATIVE = "12345678-1234-4234-8234-123456789abc"


def session(**changes):
    return dict(
        id=SID,
        profile="aoe-work",
        title="example",
        path="/work/example",
        tool="pi",
        state="live",
        command="pi-wrapper",
        **changes,
    )


def pane(**changes):
    return {
        "name": "aoe_example_01234567",
        "pane": "%12",
        "dead": "1",
        "pid": "123",
        "command": "bash",
        "kind": "agent",
        **changes,
    }


def aoe_fixture(home, detail):
    root = home / ".agent-of-empires"
    profile = root / "profiles" / detail["profile"]
    profile.mkdir(parents=True)
    (root / f".session-title-{SID}.lock").touch()
    (profile / f".instance-lifecycle-{SID}.lock").touch()
    path = profile / "sessions.json"
    path.write_text(
        json.dumps([{**detail, "project_path": detail["path"], "status": "error"}])
    )
    return root, path


class BatchTests(unittest.TestCase):
    def test_selection_excludes_everything_but_unambiguous_dead_open_sessions(self):
        cases = [
            (session(), [pane()], {}, True),
            (session(), [pane(dead="0")], {}, False),
            (session(), [pane(kind="term")], {}, False),
            (session(), [pane(kind="tool")], {}, False),
            (session(), [pane(kind="")], {}, False),
            (session(), [], {}, False),
            (session(), [pane(), pane(pane="%13")], {}, False),
            ({**session(), "state": "archived"}, [pane()], {}, False),
            ({**session(), "state": "trashed"}, [pane()], {}, False),
            (session(), [pane(name="aoe_dev_example_99999999")], {}, False),
            (session(), [pane()], {"profile": "other"}, False),
            (session(), [pane()], {"ids": ["ffffffffffffffff"]}, False),
            (session(), [pane()], {"agent": "claude"}, False),
            (session(), [pane()], {"exclude": SID}, False),
        ]
        for row, observed, filters, expected in cases:
            with self.subTest(row=row, observed=observed, filters=filters):
                self.assertEqual(
                    bool(batch.select_dead([row], observed, **filters)), expected
                )
        with self.assertRaisesRegex(ValueError, "Ambiguous"):
            batch.select_dead(
                [session(), {**session(), "id": "0123456700000000"}], [pane()]
            )

    def test_respawn_refuses_changed_or_live_panes_and_never_forces(self):
        detail = session()
        command = ["ledger", "run", "pi", "--session", "a; echo not-a-command"]
        for observed in [[], [pane(dead="0")], [pane(pid="456")], [pane(name="other")]]:
            with (
                self.subTest(observed=observed),
                patch.object(batch, "panes", return_value=observed),
                patch.object(batch, "run") as run,
            ):
                self.assertFalse(
                    batch.respawn(
                        ["tmux", "-S", "/tmp/socket"], pane(), detail, command
                    )
                )
                run.assert_not_called()
        with (
            patch.object(batch, "panes", return_value=[pane()]),
            patch.object(batch, "run") as run,
        ):
            self.assertTrue(batch.respawn(["tmux"], pane(), detail, command))
            args = run.call_args.args[0]
            self.assertNotIn("-k", args)
            self.assertEqual(shlex.split(args[-1]), command)
        with (
            patch.object(batch, "panes", return_value=[pane()]),
            patch.object(
                batch, "run", side_effect=subprocess.CalledProcessError(1, "tmux")
            ),
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                batch.respawn(["tmux"], pane(), detail, command)

    def test_lifecycle_guard_rechecks_state_and_uses_aoe_locks(self):
        with tempfile.TemporaryDirectory() as directory:
            detail = {**session(), "agent_session_id": NATIVE}
            root, path = aoe_fixture(Path(directory), detail)
            original = json.loads(path.read_text())[0]
            with batch.locked_session(detail, root):
                with self.assertRaises(BlockingIOError):
                    with batch.locked_session(detail, root):
                        self.fail("concurrent lifecycle operation acquired the lock")
            for change in [
                {"archived_at": "now"},
                {"trashed_at": "now"},
                {"status": "stopped"},
                {"lifecycle_reservation": {"operation": "launch"}},
                {"agent_session_id": "different"},
                {"project_path": "/other"},
            ]:
                path.write_text(json.dumps([{**original, **change}]))
                with self.subTest(change=change), self.assertRaises(ValueError):
                    with batch.locked_session(detail, root):
                        self.fail("changed session was accepted")
            path.write_text("[]")
            with self.assertRaises(ValueError):
                with batch.locked_session(detail, root):
                    self.fail("removed session was accepted")

    def test_transcript_identity_store_and_cwd_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            cwd = home / "worktree"
            cwd.mkdir()
            detail = {**session(), "path": str(cwd), "agent_session_id": NATIVE}
            pi_store = home / ".ledger/homes/work/pi/state/protected/sessions/cwd"
            pi_store.mkdir(parents=True)
            transcript = pi_store / f"timestamp_{NATIVE}.jsonl"
            for header, valid in [
                ({"id": NATIVE, "cwd": str(cwd)}, True),
                ({"id": "wrong", "cwd": str(cwd)}, False),
                ({"id": NATIVE, "cwd": str(home)}, False),
            ]:
                transcript.write_text(json.dumps(header) + "\n")
                if valid:
                    self.assertEqual(
                        batch.transcript_for(detail, "work", home),
                        (transcript.resolve(), NATIVE),
                    )
                else:
                    with self.assertRaises(ValueError):
                        batch.transcript_for(detail, "work", home)
            for profile in ["personal", "../work"]:
                with self.assertRaises(ValueError):
                    batch.transcript_for(detail, profile, home)
            transcript.unlink()
            outside = home / "outside.jsonl"
            outside.write_text(json.dumps({"id": NATIVE, "cwd": str(cwd)}) + "\n")
            transcript.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "escapes"):
                batch.transcript_for(detail, "work", home)
            with self.assertRaisesRegex(ValueError, "not codex"):
                batch.transcript_for({**detail, "tool": "codex"}, "work", home)

            claude_store = (
                home / ".ledger/homes/work/claude-code/state/protected/projects/cwd"
            )
            claude_store.mkdir(parents=True)
            claude = claude_store / f"{NATIVE}.jsonl"
            claude.write_text(json.dumps({"sessionId": NATIVE, "cwd": str(cwd)}) + "\n")
            self.assertEqual(
                batch.transcript_for({**detail, "tool": "claude"}, "work", home),
                (claude.resolve(), NATIVE),
            )
            with claude.open("a") as stream:
                stream.write(json.dumps({"sessionId": NATIVE, "cwd": str(home)}) + "\n")
            with self.assertRaises(ValueError):
                batch.transcript_for({**detail, "tool": "claude"}, "work", home)

    def test_native_resume_does_not_create_fresh_or_grant_permissions(self):
        for agent, flag in [("pi", "--session"), ("claude", "--resume")]:
            args = batch.resume_command(
                {**session(), "tool": agent}, "work", NATIVE, "/bin/aoe", "/bin/ledger"
            )
            self.assertEqual(
                args[-7:], ["run", agent, "--profile", "work", "--", flag, NATIVE]
            )
            self.assertNotIn("--session-dir", args)
            self.assertNotIn("--session-id", args)
            self.assertNotIn("--dangerously-skip-permissions", args)

    def test_observation_waits_full_window_and_catches_late_exit(self):
        with (
            patch.object(
                batch,
                "panes",
                side_effect=[[pane(dead="0")], [pane(dead="0")], [pane()]],
            ),
            patch.object(batch.time, "monotonic", side_effect=[0, 0, 0, 1, 1]),
            patch.object(batch.time, "sleep"),
        ):
            self.assertEqual(batch.observe(["tmux"], pane(), 15), "dead")
        with (
            patch.object(batch, "panes", return_value=[pane(dead="0")]),
            patch.object(batch.time, "monotonic", side_effect=[0, 15]),
        ):
            self.assertEqual(batch.observe(["tmux"], pane(), 15), "alive-unverified")
        with patch.object(batch, "panes", return_value=[]):
            self.assertEqual(batch.observe(["tmux"], pane(), 15), "pane-missing")

    def test_cli_preview_capture_and_apply_are_isolated(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            cwd = home / "worktree"
            cwd.mkdir()
            row = {**session(), "path": str(cwd)}
            detail = {**row, "agent_session_id": NATIVE}
            aoe_fixture(home, detail)
            store = home / ".ledger/homes/work/pi/state/protected/sessions/cwd"
            store.mkdir(parents=True)
            (store / f"timestamp_{NATIVE}.jsonl").write_text(
                json.dumps({"id": NATIVE, "cwd": str(cwd)}) + "\n"
            )
            calls = []

            def fake_run(args):
                calls.append(args)
                if "list" in args:
                    return json.dumps([row])
                if "show" in args:
                    return json.dumps(detail)
                if "capture-pane" in args:
                    return "old terminal output"
                raise AssertionError(args)

            env = {"HOME": str(home), "XDG_STATE_HOME": str(home / "state")}
            with (
                patch.dict(os.environ, env, clear=True),
                patch.object(
                    batch.shutil, "which", side_effect=lambda name: "/fake/" + name
                ),
                patch.object(batch, "panes", return_value=[pane()]),
                patch.object(batch, "run", side_effect=fake_run),
                patch.object(batch, "respawn", return_value=True) as respawn,
                patch.object(batch, "observe", return_value="alive-unverified"),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                saved_umask = os.umask(0o077)
                try:
                    args = [
                        "resume",
                        "--profile",
                        "aoe-work",
                        "--ledger-profile",
                        "work",
                    ]
                    self.assertEqual(batch.main(args), 0)
                    respawn.assert_not_called()
                    self.assertEqual(batch.main(["capture"]), 0)
                    respawn.assert_not_called()
                    self.assertEqual(batch.main(args + ["--apply"]), 0)
                    respawn.assert_called_once()
                    detail["state"] = "trashed"
                    self.assertEqual(batch.main(args + ["--apply"]), 1)
                    detail.pop("tool")
                    self.assertEqual(batch.main(args + ["--apply"]), 1)
                    respawn.assert_called_once()
                finally:
                    os.umask(saved_umask)
            reports = list((home / "state/aoe-batch").glob("*/results.jsonl"))
            self.assertEqual(len(reports), 5)
            self.assertTrue(all(p.stat().st_mode & 0o077 == 0 for p in reports))
            self.assertTrue(
                all("restart" not in call and "send-keys" not in call for call in calls)
            )


if __name__ == "__main__":
    unittest.main()
