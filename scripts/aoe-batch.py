#!/usr/bin/env python3
"""Inspect, capture, or resume dead AoE panes. See docs/guides/batch-recovery.md."""

import argparse
from contextlib import contextmanager, ExitStack
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


PANE_FORMAT = "#{session_name}|#{pane_id}|#{pane_dead}|#{pane_pid}|#{@aoe_kind}|#{pane_current_command}"


def run(argv):
    return subprocess.run(
        argv, check=True, text=True, capture_output=True, timeout=30
    ).stdout


def panes(tmux):
    result = []
    for line in run([*tmux, "list-panes", "-a", "-F", PANE_FORMAT]).splitlines():
        name, pane, dead, pid, kind, command = line.split("|", 5)
        result.append(
            dict(name=name, pane=pane, dead=dead, pid=pid, kind=kind, command=command)
        )
    return result


def select_dead(sessions, observed, profile=None, ids=(), agent=None, exclude=None):
    selected = []
    for session in sessions:
        sid = session["id"]
        if not re.fullmatch(r"[0-9a-f]{16}", sid):
            raise ValueError(f"Invalid AoE id: {sid!r}")
        if session.get("state") != "live" or sid == exclude:
            continue
        if profile and session["profile"] != profile:
            continue
        if ids and sid not in ids:
            continue
        if agent and session["tool"] != agent:
            continue
        matches = [
            pane
            for pane in observed
            if pane["name"].startswith("aoe_") and pane["name"].endswith("_" + sid[:8])
        ]
        # Refuse split/multi-window sessions rather than guessing the agent pane.
        if (
            len(matches) == 1
            and matches[0]["dead"] == "1"
            and matches[0].get("kind") == "agent"
        ):
            selected.append((session, matches[0]))
    prefixes = [session["id"][:8] for session in sessions]
    if any(prefixes.count(session["id"][:8]) != 1 for session, _ in selected):
        raise ValueError("Ambiguous AoE session name prefix")
    return selected


def transcript_for(detail, ledger_profile, home):
    if not re.fullmatch(r"[A-Za-z0-9_-]+", ledger_profile):
        raise ValueError("Invalid Ledger profile name")
    sid = str(uuid.UUID(detail.get("agent_session_id") or ""))
    tool = detail["tool"]
    cwd = Path(detail["path"]).resolve(strict=True)
    if not cwd.is_dir():
        raise ValueError("Session working directory is not a directory")
    harness = {"pi": "pi", "claude": "claude-code"}.get(tool)
    if harness is None:
        raise ValueError(
            f"Native recovery supports Pi/Claude, not {tool}; no fresh fallback"
        )
    root = home / ".ledger/homes" / ledger_profile / harness / "state/protected"
    store = root / ("sessions" if tool == "pi" else "projects")
    pattern = f"*/*_{sid}.jsonl" if tool == "pi" else f"*/{sid}.jsonl"
    files = list(store.glob(pattern))
    if len(files) != 1:
        raise ValueError(
            f"Expected one transcript for {sid} in {store}, found {len(files)}"
        )
    transcript = files[0].resolve(strict=True)
    if not transcript.is_relative_to(store.resolve(strict=True)):
        raise ValueError("Transcript escapes the selected profile's store")
    with transcript.open() as source:
        if tool == "pi":
            header = json.loads(source.readline())
            valid = (
                header.get("id") == sid and Path(header.get("cwd", "")).resolve() == cwd
            )
        else:
            cwds = {
                str(Path(row["cwd"]).resolve())
                for line in source
                if (row := json.loads(line)).get("sessionId") == sid and row.get("cwd")
            }
            valid = cwds == {str(cwd)}
    if not valid:
        raise ValueError(
            "Transcript identity/working directory does not match the AoE session"
        )
    return transcript, sid


def resume_command(detail, ledger_profile, sid, aoe, ledger):
    env = {
        "AOE_INSTANCE_ID": detail["id"],
        "AOE_PROFILE": detail["profile"],
        "AOE_REPORT_PROFILE": detail["profile"],
        "AOE_REPORT_AGENT": detail["tool"],
        "AOE_REPORT_BIN": aoe,
        "AOE_USAGE_BIN": aoe,
        "AOE_SESSION_SOURCE": str(uuid.uuid4()),
        "AOE_SESSION_ROOT_ONLY": "0",
        "AOE_SESSION_ID_FILE": f"/tmp/aoe-hooks-{os.getuid()}/{detail['id']}/session_id",
    }
    flag = "--session" if detail["tool"] == "pi" else "--resume"
    return [
        "env",
        *[f"{k}={v}" for k, v in env.items()],
        ledger,
        "run",
        detail["tool"],
        "--profile",
        ledger_profile,
        "--",
        flag,
        sid,
    ]


def same_dead_pane(expected, observed):
    return any(
        all(row[key] == expected[key] for key in ("name", "pane", "pid"))
        and row["dead"] == "1"
        and row.get("kind") == "agent"
        for row in observed
    )


@contextmanager
def locked_session(detail, aoe_dir=None):
    """Use existing AoE v1.18 locks; refuse unknown layouts rather than invent locks."""
    profile = detail["profile"]
    sid = detail["id"]
    if not re.fullmatch(r"[A-Za-z0-9_-]+", profile):
        raise ValueError("Invalid AoE profile name")
    candidates = (
        [Path(aoe_dir)]
        if aoe_dir
        else [
            Path.home() / ".agent-of-empires",
            Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config")))
            / "agent-of-empires",
        ]
    )
    roots = {
        p.resolve()
        for p in candidates
        if (p / "profiles" / profile / "sessions.json").is_file()
    }
    if len(roots) != 1:
        raise ValueError("AoE state directory is missing/ambiguous; specify --aoe-dir")
    root = roots.pop()
    profile_dir = root / "profiles" / profile
    with ExitStack() as stack:
        for path in [
            root / f".session-title-{sid}.lock",
            profile_dir / f".instance-lifecycle-{sid}.lock",
        ]:
            lock = stack.enter_context(path.open("rb"))
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        rows = json.loads((profile_dir / "sessions.json").read_text())
        matches = [row for row in rows if row.get("id") == sid]
        if len(matches) != 1:
            raise ValueError("Session removed or moved since selection")
        row = matches[0]
        if any(
            row.get(key)
            for key in ("archived_at", "trashed_at", "lifecycle_reservation")
        ) or row.get("status") in ("stopped", "starting", "creating", "deleting"):
            raise ValueError(
                "Session is shelved, intentionally stopped, or has an operation in progress"
            )
        for stored, public in [
            ("project_path", "path"),
            ("agent_session_id", "agent_session_id"),
            ("tool", "tool"),
            ("command", "command"),
            ("title", "title"),
        ]:
            if row.get(stored) != detail.get(public):
                raise ValueError(f"Session {public} changed since selection")
        yield


def respawn(tmux, pane, detail, command):
    if not same_dead_pane(pane, panes(tmux)):
        return False
    # Without -k, tmux atomically refuses a pane that another operator revived.
    run(
        [
            *tmux,
            "respawn-pane",
            "-t",
            pane["pane"],
            "-c",
            detail["path"],
            shlex.join(command),
        ]
    )
    return True


def observe(tmux, expected, seconds):
    deadline = time.monotonic() + seconds
    while True:
        matches = [p for p in panes(tmux) if p["pane"] == expected["pane"]]
        if len(matches) != 1 or matches[0]["name"] != expected["name"]:
            return "pane-missing"
        if matches[0]["dead"] == "1":
            return "dead"
        if time.monotonic() >= deadline:
            return "alive-unverified"
        time.sleep(max(0, min(0.5, deadline - time.monotonic())))


def capture(tmux, pane):
    return run([*tmux, "capture-pane", "-p", "-S", "-200", "-t", pane["pane"]])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["status", "capture", "resume"])
    parser.add_argument(
        "--profile", help="AoE profile; status/capture default to all profiles"
    )
    parser.add_argument(
        "--ledger-profile", help="Explicit original Ledger profile for native resume"
    )
    parser.add_argument(
        "--id", action="append", default=[], help="Exact AoE id, repeatable"
    )
    parser.add_argument("--agent", choices=["pi", "claude", "codex"])
    parser.add_argument(
        "--apply", action="store_true", help="Actually resume; otherwise preview only"
    )
    parser.add_argument(
        "--limit", type=int, default=0, help="Maximum attempts; 0 means all selected"
    )
    parser.add_argument(
        "--observe-seconds",
        type=float,
        default=15,
        help="Post-launch observation window",
    )
    parser.add_argument(
        "--socket",
        default=os.environ.get("AOE_TMUX_SOCKET"),
        help="tmux socket path (-S)",
    )
    parser.add_argument(
        "--aoe-dir", help="AoE v1.18 state directory, if not in its standard location"
    )
    args = parser.parse_args(argv)
    if args.limit < 0 or not 1 <= args.observe_seconds <= 300:
        parser.error("--limit must be nonnegative; --observe-seconds must be 1..300")
    if args.operation == "resume" and not (args.profile and args.ledger_profile):
        parser.error("resume requires both --profile and --ledger-profile")
    if args.apply and args.operation != "resume":
        parser.error("--apply only applies to resume")
    if os.environ.get("AOE_DAEMON_URL"):
        parser.error("Local recovery only; unset AOE_DAEMON_URL")
    for sid in args.id:
        if not re.fullmatch(r"[0-9a-f]{16}", sid):
            parser.error("--id requires a full 16-character AoE id")

    aoe = shutil.which("aoe")
    ledger = shutil.which("ledger")
    if not aoe or (args.operation == "resume" and not ledger):
        parser.error("aoe (and ledger for resume) must be on PATH")
    tmux = ["tmux", "-S", args.socket] if args.socket else ["tmux"]
    os.umask(0o077)
    root = (
        Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
        / "aoe-batch"
    )
    root.mkdir(parents=True, exist_ok=True)
    # Retain the lock descriptor for the entire run, including observation.
    with (root / "batch.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        output = Path(
            tempfile.mkdtemp(prefix=time.strftime("%Y%m%d-%H%M%S-"), dir=root)
        )
        sessions = json.loads(run([aoe, "list", "--all", "--state", "live", "--json"]))
        observed = panes(tmux)
        (output / "inventory.json").write_text(
            json.dumps(dict(sessions=sessions, panes=observed), indent=2)
        )
        selected = select_dead(
            sessions,
            observed,
            args.profile,
            args.id,
            args.agent,
            os.environ.get("AOE_INSTANCE_ID"),
        )
        if args.limit:
            selected = selected[: args.limit]
        print(f"{len(selected)} dead open pane(s); evidence: {output}", flush=True)
        failed = False
        for session, pane in selected:
            result = dict(id=session["id"], title=session["title"], pane=pane["pane"])
            try:
                if args.operation == "status":
                    result["result"] = "dead"
                else:
                    (output / f"{session['id']}-before.txt").write_text(
                        capture(tmux, pane)
                    )
                    result["result"] = "captured"
                if args.operation == "resume":
                    detail = json.loads(
                        run(
                            [
                                aoe,
                                "-p",
                                session["profile"],
                                "session",
                                "show",
                                session["id"],
                                "--json",
                            ]
                        )
                    )
                    (output / f"{session['id']}-detail.json").write_text(
                        json.dumps(detail, indent=2)
                    )
                    if (
                        any(
                            detail[key] != session[key]
                            for key in ("id", "profile", "path", "tool", "command")
                        )
                        or detail.get("state") != "live"
                    ):
                        raise ValueError(
                            "Session changed since selection; rerun the preview"
                        )
                    transcript, sid = transcript_for(
                        detail, args.ledger_profile, Path.home()
                    )
                    command = resume_command(
                        detail, args.ledger_profile, sid, aoe, ledger
                    )
                    result.update(
                        transcript=str(transcript), native_id=sid, command=command
                    )
                    result["result"] = "would-resume"
                    with locked_session(detail, args.aoe_dir):
                        transcript_for(detail, args.ledger_profile, Path.home())
                        started = args.apply and respawn(tmux, pane, detail, command)
                    if args.apply:
                        if started:
                            result["result"] = observe(tmux, pane, args.observe_seconds)
                            (output / f"{session['id']}-after.txt").write_text(
                                capture(tmux, pane)
                            )
                            failed |= result["result"] != "alive-unverified"
                        else:
                            result["result"] = "skipped-changed-pane"
            except (
                ValueError,
                KeyError,
                TypeError,
                AttributeError,
                OSError,
                subprocess.SubprocessError,
            ) as error:
                result.update(result="blocked", error=str(error))
                failed = True
            with (output / "results.jsonl").open("a") as stream:
                stream.write(json.dumps(result) + "\n")
            print(
                f"{result['result']:20} {session['profile']} {session['id']} {session['title']}"
                + (f"\n  {result['error']}" if "error" in result else ""),
                flush=True,
            )
        if args.operation == "resume":
            print(
                "No prompts sent. Alive is not ready: inspect panes for approvals/authentication."
            )
        return 1 if failed else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (
        ValueError,
        KeyError,
        TypeError,
        AttributeError,
        OSError,
        subprocess.SubprocessError,
    ) as error:
        print(f"aoe-batch: {error}", file=sys.stderr)
        sys.exit(1)
