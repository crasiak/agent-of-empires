"""Own only fixed controlled fixture processes. This is not a native launch adapter."""
import json
import os
from pathlib import Path
import secrets
import select
import signal
import stat
import subprocess
import sys
import tempfile
import time
import shutil

HERE = Path(__file__).resolve().parent
SCENARIOS = json.loads((HERE / "aoe-afk-observer-scenarios.json").read_text())
MAX_BYTES, MAX_ROWS, AUDIT_CAP = 262144, 1024, 4096
FLAGS = {"teardown", "shutdown", "fault", "cancelled", "opened", "attributed", "nativeDeferred", "waitArmed", "runCompleted"}
EVENTS = {"request", "resolved", "rejected", "close", "closed", "close_failed", "attach", "attached", "attach_failed", "failed", "failed_returned", "failed_threw", "opened", "question_closed", "reply", "teardown", "truncated"}
OPS = {"poll", "ack", "runtime_ack", "question_recover", "runtime", "other"}
KINDS = {"none", "reserve", "stop", "settle", "question_admit", "question_intent", "question_outcome", "read", "record", "apply", "checkpoint", "nudge", "contribution_intent", "other"}
CALLERS = {"poll", "before_request", "before_tool", "turn_end", "settled", "shutdown", "failed_callback", "unknown", "other"}
PHASES = {"refresh": {"start", "join", "resolved", "rejected", "threw"}, "cleanup_stop": {"entry", "returned", "rejected", "dispatch", "reply_resolved", "reply_rejected", "threw"}}


def integer(value, maximum):
    return type(value) is int and 0 <= value <= maximum


def writer(value):
    return value is None or (type(value) is dict and set(value) == {"invalid", "truncated", "events", "bytes"}
        and type(value["invalid"]) is bool and type(value["truncated"]) is bool
        and integer(value["events"], MAX_ROWS) and integer(value["bytes"], MAX_BYTES))


def audit_receipt(raw, nonce):
    try:
        if not raw.endswith(b"\n") or raw.count(b"\n") != 1 or len(raw) > AUDIT_CAP:
            return None
        value = json.loads(raw.decode("utf-8", errors="strict"))
        expected = FLAGS | {"version", "nonce", "calls", "trace", "probes", "fixture"}
        if type(value) is not dict or set(value) != expected or type(value["version"]) is not int or value["version"] != 2 or value["nonce"] != nonce:
            return None
        if not all(type(value[key]) is bool for key in FLAGS) or not integer(value["calls"], 65535):
            return None
        if not writer(value["trace"]) or not writer(value["probes"]):
            return None
        fixture = value["fixture"]
        if type(fixture) is not dict or set(fixture) != {"kind", "workCompleted", "shutdownCompleted", "observationInstalled"}:
            return None
        if fixture["kind"] != "controlled" or not all(type(fixture[key]) is bool for key in fixture if key != "kind"):
            return None
        return value
    except (ValueError, UnicodeError, TypeError, KeyError):
        return None


def valid_row(row, probes):
    if type(row) is not dict:
        return False
    if probes:
        expected = {"seq", "type", "phase", "call", "parent", "caller", "bridge", "request", "kind", "teardown"}
        if set(row) != expected or row["type"] not in PHASES or row["phase"] not in PHASES[row["type"]] or row["caller"] not in CALLERS:
            return False
        if not integer(row["call"], 65535) or row["call"] == 0 or not integer(row["parent"], 65535):
            return False
    else:
        expected = {"seq", "event", "bridge", "request", "op", "kind", "duration", "teardown"}
        if set(row) != expected or row["event"] not in EVENTS or row["op"] not in OPS or not integer(row["duration"], 86400000):
            return False
    return (integer(row["seq"], MAX_ROWS) and row["seq"] > 0 and integer(row["bridge"], 65535)
        and integer(row["request"], 65535) and row["kind"] in KINDS and type(row["teardown"]) is bool)


def read_log(root, name, identity, probes=False):
    fd = None
    try:
        current = root.lstat()
        if ((current.st_dev, current.st_ino) != identity or not stat.S_ISDIR(current.st_mode)
            or stat.S_IMODE(current.st_mode) != 0o700 or current.st_uid != os.getuid()):
            return [None], False, 0
        path = root / name
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600 or info.st_uid != os.getuid() or info.st_nlink != 1:
            return [None], False, 0
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            return [None], False, 0
        body = bytearray()
        while len(body) <= MAX_BYTES:
            chunk = os.read(fd, min(65536, MAX_BYTES + 1 - len(body)))
            if not chunk:
                break
            body.extend(chunk)
        after = os.fstat(fd)
        valid = len(body) <= MAX_BYTES and opened.st_size == after.st_size == len(body)
        rows = []
        lines = bytes(body).splitlines(keepends=True)
        if len(lines) > MAX_ROWS:
            valid = False
        for line in lines[:MAX_ROWS + 1]:
            try:
                row = json.loads(line.decode("utf-8", errors="strict"))
                if not valid_row(row, probes):
                    raise ValueError("invalid schema")
                canonical = (json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n").encode()
                if line != canonical:
                    valid = False
                rows.append(row)
            except (ValueError, UnicodeError, TypeError, KeyError):
                valid = False
                rows.append(None)
        if len(body) > MAX_BYTES:
            rows.append(None)
        if (root.lstat().st_dev, root.lstat().st_ino) != identity:
            valid = False
        return rows, valid, len(body)
    except FileNotFoundError:
        return None, False, 0
    except OSError:
        return [None], False, 0
    finally:
        if fd is not None:
            os.close(fd)


class Resources:
    def __init__(self, scenario):
        self.scenario = scenario
        self.root = None
        self.identity = None
        self.fds = set()
        self.children = []
        self.pipe_calls = 0
        self.root_removed = False

    def make_root(self):
        self.root = Path(tempfile.mkdtemp(prefix="aoe-diagnostic-controlled-"))
        info = self.root.lstat()
        self.identity = (info.st_dev, info.st_ino)
        os.chmod(self.root, 0o700)
        return self.root

    def pipe(self):
        self.pipe_calls += 1
        if self.scenario == "setup-pipe-failure" and self.pipe_calls == 2:
            raise OSError("controlled allocation failure")
        pair = os.pipe()
        self.fds.update(pair)
        return pair

    def close_fd(self, fd):
        if fd in self.fds:
            self.fds.remove(fd)
            try:
                os.close(fd)
            except OSError:
                pass

    def spawn(self, *args, **kwargs):
        child = subprocess.Popen(*args, **kwargs)
        self.children.append(child)
        return child

    def cleanup(self):
        for owned in self.children:
            if owned.poll() is not None:
                continue
            try:
                owned.terminate()
                owned.wait(timeout=0.3)
            except subprocess.TimeoutExpired:
                owned.kill()
                try:
                    owned.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    pass
            except ProcessLookupError:
                owned.wait(timeout=3)
        for fd in tuple(self.fds):
            self.close_fd(fd)
        for owned in self.children:
            if owned.stdin is not None and not owned.stdin.closed:
                owned.stdin.close()
        stopped = all(owned.poll() is not None for owned in self.children)
        if self.root is None:
            self.root_removed = True
        elif stopped and not self.root_removed:
            try:
                info = self.root.lstat()
                if stat.S_ISDIR(info.st_mode) and (info.st_dev, info.st_ino) == self.identity:
                    shutil.rmtree(self.root)
                    self.root_removed = True
            except FileNotFoundError:
                self.root_removed = True
        return stopped and not self.fds and self.root_removed


def _run(scenario, resources):
    if scenario not in SCENARIOS:
        raise ValueError("unsupported scenario")
    root = resources.make_root()
    identity = resources.identity
    dirs = ["home", "xdg", "cache", "data", "pi", "tmp", "work"]
    for name in dirs:
        (root / name).mkdir(mode=0o700)
    nonce = secrets.token_hex(16)
    ar, aw = resources.pipe()
    cr, cw = resources.pipe()
    dr, dw = resources.pipe()
    hr, hw = resources.pipe()
    os.set_blocking(ar, False)
    env = {"PATH": os.path.dirname(os.environ["AOE_FIXTURE_NODE"]) + ":/usr/bin:/bin", "HOME": str(root / "home"),
        "XDG_CONFIG_HOME": str(root / "xdg"), "XDG_CACHE_HOME": str(root / "cache"), "XDG_DATA_HOME": str(root / "data"),
        "PI_CODING_AGENT_DIR": str(root / "pi"), "TMPDIR": str(root / "tmp"), "TERM": "dumb", "SHELL": "/bin/bash", "LANG": "en_US.UTF-8",
        "AOE_FIXTURE_ROOT": str(root), "AOE_FIXTURE_TRACE": str(root / "trace.jsonl"), "AOE_FIXTURE_PROBES": str(root / "probes.jsonl"),
        "AOE_FIXTURE_AUDIT_NONCE": nonce, "AOE_FIXTURE_AUDIT_FD": str(aw), "AOE_FIXTURE_CONTROL_FD": str(cw), "AOE_FIXTURE_COMMANDS_FD": str(dr)}
    child = holder = None
    audit = bytearray()
    control = bytearray()
    holder_control = bytearray()
    events = []
    audit_eof = control_eof = holder_ready = released = exit_sent = main_seen = held_observed = False
    timed_out = escalated = control_invalid = False
    failures = []
    deadline = time.monotonic() + 15
    kill_deadline = hard_deadline = None
    streams = {ar: "audit", cr: "control"}

    def audit_withheld():
        nonlocal audit_eof
        if audit_eof:
            return False
        for _ in range(64):
            try:
                chunk = os.read(ar, 8192)
            except BlockingIOError:
                events.append("audit_withheld")
                return True
            if not chunk:
                audit_eof = True
                events.append("audit_eof")
                streams.pop(ar, None)
                resources.close_fd(ar)
                return False
            audit.extend(chunk[:max(0, AUDIT_CAP + 1 - len(audit))])
        return False

    try:
        try:
            executable = "/nonexistent-controlled-fixture-node" if scenario == "spawn-failure" else os.environ["AOE_FIXTURE_NODE"]
            child = resources.spawn([executable, str(HERE / "aoe-afk-observer-controlled-fixture.mjs"), scenario],
                cwd=root / "work", env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, pass_fds=(aw, cw, dr))
        except OSError:
            return {"error": "spawn_failed"}
        if scenario in {"holder", "holder-ignored", "holder-no-fd", "holder-spawn-failure"}:
            helper = "import os,sys,signal; fd=int(sys.argv[1]); os.write(fd,b'holding\\n'); sys.stdin.readline(); " + ("os.write(fd,b'ignored\\n'); signal.pause()" if scenario == "holder-ignored" else "os.write(fd,b'released\\n'); sys.exit(0)")
            try:
                interpreter = "/nonexistent-controlled-holder" if scenario == "holder-spawn-failure" else sys.executable
                holder = resources.spawn([interpreter, "-c", helper, str(hw)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, pass_fds=(hw,) if scenario == "holder-no-fd" else (aw, hw), env={"PATH": "/usr/bin:/bin"})
            except OSError:
                return {"error": "holder_spawn_failed"}
            streams[hr] = "holder"
        else:
            resources.close_fd(hr)
        for fd in (aw, cw, dr, hw):
            resources.close_fd(fd)
        while True:
            now = time.monotonic()
            if child.poll() is not None and not main_seen:
                main_seen = True
                events.append("main_exit")
                if holder is not None and holder_ready and holder.poll() is None:
                    held_observed = audit_withheld()
                    if not held_observed:
                        failures.append("holder_invalid")
                    holder.stdin.write(b"release\n")
                    holder.stdin.flush()
                    holder.stdin.close()
                    events.append("holder_release")
                    deadline = now + 3
                else:
                    if holder is not None:
                        failures.append("holder_invalid")
                    deadline = min(deadline, now + 3)
            if now >= deadline and not timed_out:
                timed_out = True
                failures.append("supervisor_deadline")
                for owned in (child, holder):
                    if owned is not None and owned.poll() is None:
                        owned.terminate()
                kill_deadline = now + 0.3
            if kill_deadline is not None and now >= kill_deadline:
                for owned in (child, holder):
                    if owned is not None and owned.poll() is None:
                        owned.kill()
                        escalated = True
                kill_deadline = None
                hard_deadline = now + 3
            if hard_deadline is not None and now >= hard_deadline:
                failures.append("cleanup_incomplete")
                break
            if child.poll() is not None and (holder is None or holder.poll() is not None) and audit_eof and control_eof:
                break
            readable, _, _ = select.select(list(streams), [], [], 0.05)
            for fd in readable:
                data = os.read(fd, 8192)
                kind = streams[fd]
                if not data:
                    del streams[fd]
                    resources.close_fd(fd)
                    if kind == "audit":
                        audit_eof = True
                        events.append("audit_eof")
                    elif kind == "control":
                        control_eof = True
                    continue
                if kind == "audit":
                    audit.extend(data[:max(0, AUDIT_CAP + 1 - len(audit))])
                elif kind == "holder":
                    holder_control.extend(data)
                    if len(holder_control) > 64:
                        control_invalid = True
                        holder_control = holder_control[:64]
                    while b"\n" in holder_control:
                        line, _, remainder = holder_control.partition(b"\n")
                        holder_control = bytearray(remainder)
                        if line == b"holding" and not holder_ready:
                            holder_ready = True
                            events.append("holder_ready")
                        elif line in (b"released", b"ignored") and "holder_release" in events:
                            label = "holder_" + line.decode()
                            if label in events:
                                control_invalid = True
                            events.append(label)
                            if line == b"ignored":
                                deadline = time.monotonic() + 0.3
                        else:
                            control_invalid = True
                else:
                    control.extend(data)
                    if len(control) > 512:
                        control_invalid = True
                        control = control[:512]
                    while b"\n" in control:
                        line, _, remainder = control.partition(b"\n")
                        line = bytes(line)
                        control = bytearray(remainder)
                        if line not in {b"ready", b"completed", b"not_reached", b"ignored", b"audited_hang"} or line.decode() in events:
                            control_invalid = True
                            continue
                        events.append(line.decode())
                        if line in {b"ignored", b"audited_hang"}:
                            deadline = time.monotonic() + 0.3
                if "ready" in events and not released and (holder is None or holder_ready):
                    os.write(dw, b"release\n")
                    released = True
                    deadline = time.monotonic() + 10
                if ("completed" in events or "not_reached" in events) and not exit_sent:
                    os.write(dw, b"exit\n")
                    exit_sent = True
                    deadline = time.monotonic() + 3
        reaped = child.returncode is not None
        holder_reaped = holder is None or holder.returncode is not None
        cleanup = reaped and holder_reaped
        control_invalid = bool(control or holder_control or control_invalid)
        if control_invalid:
            failures.append("control_invalid")
        receipt = audit_receipt(bytes(audit), nonce) if audit_eof else None
        if receipt is None:
            failures.append("audit_invalid")
        if scenario == "trace-fifo" and (root / "trace.jsonl").exists():
            (root / "trace.jsonl").unlink()
            os.mkfifo(root / "trace.jsonl", 0o600)
        trace, trace_valid, trace_bytes = read_log(root, "trace.jsonl", identity)
        probes, probe_valid, probe_bytes = read_log(root, "probes.jsonl", identity, True)
        if not trace_valid:
            failures.append("trace_invalid")
        if not probe_valid:
            failures.append("probes_invalid")
        if set(os.listdir(root)) - set(dirs) - {"trace.jsonl", "probes.jsonl", "teardown"}:
            failures.append("unexpected_entry")
        physical = bool(receipt and receipt["trace"] and receipt["probes"] and trace_valid and probe_valid
            and receipt["trace"]["bytes"] == trace_bytes and receipt["trace"]["events"] == len(trace)
            and receipt["probes"]["bytes"] == probe_bytes and receipt["probes"]["events"] == len(probes))
        if not physical:
            failures.append("writer_receipt_mismatch")
        healthy = bool(receipt and receipt["trace"] and receipt["probes"] and all(
            not receipt[name][flag] for name in ("trace", "probes") for flag in ("invalid", "truncated")))
        if not healthy:
            failures.append("writer_invalid")
        endpoint = "qualified" if released and "completed" in events and not control_invalid else "not_reached"
        lifecycle = bool(receipt and receipt["teardown"] and receipt["shutdown"] and not receipt["fault"]
            and all(receipt["fixture"][key] for key in ("workCompleted", "shutdownCompleted", "observationInstalled")))
        completed = cleanup and audit_eof and control_eof and not failures and physical and lifecycle
        code = child.returncode
        waitpid = None if not reaped else {"code": code if code >= 0 else None, "signal": signal.Signals(-code).name if code < 0 else None}
        result = {"version": 1, "endpoint": endpoint, "trace": trace, "probes": probes,
            "completion": {"waitpid": waitpid, "audit": {"completed": completed, "trace": receipt["trace"] if receipt else None, "probes": receipt["probes"] if receipt else None}},
            "process": {"main_reaped": reaped, "holder_reaped": holder_reaped, "audit_eof": audit_eof, "control_eof": control_eof,
                "deadline": timed_out, "escalated": escalated, "cleanup_complete": cleanup, "held_fd_observed": held_observed},
            "ingestion": {"audit_valid": receipt is not None, "trace_valid": trace_valid, "probes_valid": probe_valid, "control_valid": not control_invalid},
            "events": events, "failures": sorted(set(failures)), "retained_root": str(root) if not cleanup else None}
        return result
    finally:
        resources.cleanup()


def run(scenario):
    resources = Resources(scenario)
    try:
        result = _run(scenario, resources)
    except Exception:
        result = {"error": "setup_failed" if not resources.children else "supervisor_failed"}
    finally:
        cleanup = resources.cleanup()
    main = resources.children[0] if resources.children else None
    if "error" in result:
        code = main.returncode if main is not None else None
        result.update({"version": 1, "completion": {"waitpid": None if code is None else {"code": code if code >= 0 else None,
            "signal": signal.Signals(-code).name if code < 0 else None}, "audit": {"completed": False}},
            "process": {"main_reaped": main is not None and main.returncode is not None, "cleanup_complete": cleanup}})
    else:
        result["process"]["cleanup_complete"] = result["process"]["cleanup_complete"] and cleanup
        if not cleanup:
            result["completion"]["audit"]["completed"] = False
            result["failures"].append("cleanup_incomplete")
    result["process"].update({"root_removed": resources.root_removed, "open_fds_left": len(resources.fds)})
    result["retained_root"] = None if cleanup or resources.root is None else str(resources.root)
    return result


if __name__ == "__main__":
    try:
        print(json.dumps(run(sys.argv[1]), ensure_ascii=False, separators=(",", ":")))
    except Exception:
        print('{"error":"controlled_supervisor_failed"}')
        sys.exit(1)
