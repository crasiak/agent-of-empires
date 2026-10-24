"""Native controller and terminal guardian. Production entry is the separately approved run command."""
import fcntl
import json
import os
from pathlib import Path
import re
import select
import signal
import socket
import stat
import subprocess
import sys
import time
from types import ModuleType

# Compile only the fixed source helper, never an ignored timestamp-matched cache.
_adapter_path = Path(__file__).with_name("aoe-afk-native.py")
_adapter_fd = os.open(_adapter_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
try:
    _meta = os.fstat(_adapter_fd)
    if not stat.S_ISREG(_meta.st_mode) or _meta.st_uid != os.getuid() or _meta.st_nlink != 1 or _meta.st_size > 128 * 1024:
        raise ValueError("unsafe adapter source")
    _source = os.read(_adapter_fd, 128 * 1024 + 1)
    _after = os.fstat(_adapter_fd)
    if len(_source) != _meta.st_size or (_meta.st_size, _meta.st_mtime_ns) != (_after.st_size, _after.st_mtime_ns):
        raise ValueError("adapter source changed")
finally:
    os.close(_adapter_fd)
a = ModuleType("native_adapter")
a.__file__ = str(_adapter_path)
exec(compile(_source, str(_adapter_path), "exec"), a.__dict__)
del _source


def pause():
    select.select([], [], [], 0.01)


def await_value(read, deadline):
    while True:
        value = read()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise ValueError("observable predicate deadline")
        pause()


def optional(path, limit=a.LIMIT, private=True):
    try:
        return a.read_json(path, limit, private)
    except FileNotFoundError:
        return None


def absent(path):
    try:
        os.lstat(path)
        return False
    except FileNotFoundError:
        return True


def beneath(root, path):
    root, path = Path(root), Path(path)
    path.relative_to(root)
    for parent in [path.parent, *path.parent.parents]:
        s = parent.lstat()
        if stat.S_ISLNK(s.st_mode) or not stat.S_ISDIR(s.st_mode):
            raise ValueError("capture parent is not a real directory")
        if parent == root:
            break
    return path


def capture(root, attempt, preview, approved_digest, clock):
    """No RPC, pruning, authority publication, repair or transcript initialization."""
    root = Path(root)
    bootstrap = a.read_json(root / "bootstrap.json")
    b = bootstrap["binding"]
    if bootstrap["app_dir"] != attempt["app"] or b["profile"] != "default":
        raise ValueError("foreign capture namespace")
    app = beneath(root, attempt["app"])
    profile = beneath(root, app / "afk-authority-v3/profiles/default")
    transport = beneath(root, profile / "transport" / b["instance_id"])
    lock_path = profile / "writer.lock"
    lock = os.open(lock_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    lock_stat = os.fstat(lock)
    if not stat.S_ISREG(lock_stat.st_mode) or lock_stat.st_uid != os.getuid() or lock_stat.st_nlink != 1:
        os.close(lock)
        raise ValueError("unsafe authority lock")
    deadline = time.monotonic() + 1
    try:
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError("capture profile lock deadline")
                pause()
        def raw(path, limit=a.LIMIT, private=True):
            return a.read_bytes(beneath(root, path), limit, private)
        snapshot_bytes = raw(profile / "snapshot.json", 32 * 1024 * 1024)
        registry_bytes = raw(app / "profiles/default/sessions.json", 8 * 1024 * 1024, False)
        native = a.read_json(root / "endpoint-native.json")
        native_file = beneath(root / "sessions", native["file"])
        native_bytes = raw(native_file, 8 * 1024 * 1024, False)
        if not native_bytes.endswith(b"\n"):
            raise ValueError("incomplete native source")
        rows = [json.loads(line) for line in native_bytes.splitlines()]
        if len(rows) > 8193:
            raise ValueError("native source row cap")
        hello = optional(transport / "usage-reporter.json")
        lease = optional(transport / "usage-lease.json")
        runtime_ack = optional(transport / "runtime-ack.json")
        runtime_probe = optional(transport / "runtime-probe.json")
        pending_absent = absent(profile / "publication.pending.json")
        unresolved_absent = absent(transport / "usage-unresolved.json")
        scratch = beneath(root, root / "work/scratch.txt")
        scratch_absent = absent(scratch)
        now = clock()
        held_lock = lock_path.lstat()
        stable = ((held_lock.st_dev, held_lock.st_ino) == (lock_stat.st_dev, lock_stat.st_ino)
                  and snapshot_bytes == raw(profile / "snapshot.json", 32 * 1024 * 1024)
                  and registry_bytes == raw(app / "profiles/default/sessions.json", 8 * 1024 * 1024, False)
                  and native_bytes == raw(native_file, 8 * 1024 * 1024, False)
                  and hello == optional(transport / "usage-reporter.json") and lease == optional(transport / "usage-lease.json")
                  and runtime_ack == optional(transport / "runtime-ack.json") and runtime_probe == optional(transport / "runtime-probe.json")
                  and pending_absent == absent(profile / "publication.pending.json")
                  and unresolved_absent == absent(transport / "usage-unresolved.json")
                  and scratch_absent == absent(scratch))
        return {"bootstrap": bootstrap, "snapshot": json.loads(snapshot_bytes), "registry": json.loads(registry_bytes),
                "native_rows": rows, "native_file": str(native_file), "hello": hello, "lease": lease, "clock": now,
                "runtime_ack": runtime_ack, "runtime_probe": runtime_probe,
                "pending_absent": pending_absent, "unresolved_absent": unresolved_absent, "stable": stable,
                "scratch_absent": scratch_absent, "scratch_path": str(scratch), "work": str(root / "work"),
                "approved_preview_digest": approved_digest, "preview_operation": preview["operation_id"],
                "ready": a.read_json(root / "ready.json"), "apply": a.read_json(root / "apply-result.json"),
                "attempt": a.read_json(root / "attempt.json"), "approval": a.read_json(root / "apply-admitted.json"),
                "work_identity": [os.stat(root / "work").st_dev, os.stat(root / "work").st_ino]}
    finally:
        os.close(lock)


def initialize(root, manifest, role, ordinal):
    root = Path(root)
    root.mkdir(mode=0o700)
    for name in ["home", "config", "cache", "data", "state", "agent", "tmp", "work", "sessions", "bin"]:
        (root / name).mkdir(mode=0o700)
    # Only reviewed tool links, never ambient PATH or credentials.
    for name in ["tmux", "fd", "rg"]:
        (root / "bin" / name).symlink_to(manifest["tools"][name]["path"])
    (root / "bin/pi").symlink_to(Path(manifest["root"]) / "pi")
    attempt = {**manifest["experiment"], "manifest": a.digest(manifest), "role": role, "ordinal": ordinal,
               "app": str(root / "config" / manifest["config"]["app_name"]), "root": str(root)}
    app = Path(attempt["app"])
    app.mkdir(mode=0o700)
    (app / "profiles").mkdir(mode=0o700)
    (app / "profiles/default").mkdir(mode=0o700)
    a.publish_bytes(app / "config.toml", b'default_profile = "default"\n[systemd]\nenabled = false\n[updates]\nupdate_check_mode = "off"\n[session]\nrestart_wake_message = ""\nusage_tracking = false\n')
    a.publish(root / "attempt.json", attempt, 0o400)
    a.publish(root / "agent/settings.json", {"compaction": {"enabled": False},
              "retry": {"enabled": False, "provider": {"maxRetries": 0}}, "cacheWarming": "off", "quietStartup": True})
    return attempt


def preview_approval(manifest, attempt, preview):
    return {"kind": a.KIND, "manifest": a.digest(manifest), "role": attempt["role"], "ordinal": attempt["ordinal"],
            "operation_id": preview["operation_id"], "preview_digest": preview["digest"], "approve_apply": True}


class NativeIO:
    def __init__(self, root, manifest, pipe):
        self.root, self.manifest, self.pipe = Path(root), manifest, pipe
        self.env = a.isolated_env(root, manifest)
        self.deadline = time.monotonic() + manifest["attempt_seconds"]
        self.pane = None

    def command(self, args, timeout=10):
        remaining = min(timeout, self.deadline - time.monotonic())
        if remaining <= 0:
            raise ValueError("attempt deadline")
        return a.bounded_command(args, self.env, self.root / "work", remaining)

    def aoe(self, *args):
        return self.command([self.manifest["tools"]["aoe"]["path"], "-p", "default", *args])

    def tmux(self, *args):
        return self.command([self.manifest["tools"]["tmux"]["path"], "-S", self.env["AOE_TMUX_SOCKET"], *args])

    def marker(self, name, seconds):
        return await_value(lambda: optional(self.root / name), min(self.deadline, time.monotonic() + seconds))

    def terminal(self):
        self.pipe.settimeout(min(5, max(0.01, self.deadline - time.monotonic())))
        line = recv_line(self.pipe)
        value = json.loads(line)
        self.pane = value["pane"]
        return value

    def control(self, value):
        if value not in ("cancel", "off", "shutdown"):
            raise ValueError("invalid native control")
        self.pipe.sendall(value.encode() + b"\n")

    def arm(self):
        if not self.pane or not re.fullmatch(r"%[0-9]+", self.pane):
            raise ValueError("unowned pane")
        self.tmux("send-keys", "-t", self.pane, "-l", "/aoe-native-complete")
        self.tmux("send-keys", "-t", self.pane, "Enter")

    def capture(self, attempt, preview):
        return capture(self.root, attempt, preview, preview["digest"],
                       lambda: json.loads(self.command([self.manifest["tools"]["aoe"]["path"], "__afk-observation-clock"])))

    def wait_capture_change(self, captured, deadline):
        b = captured["bootstrap"]["binding"]
        path = Path(captured["bootstrap"]["app_dir"]) / "afk-authority-v3/profiles/default/transport" / b["instance_id"] / "usage-lease.json"
        await_value(lambda: optional(path) != captured["lease"], deadline)

    def endpoint(self, captured):
        output = self.command([self.manifest["tools"]["node"]["path"], str(a.HERE / "aoe-afk-native-report.mjs"), str(self.root), "endpoint"])
        return json.loads(output)


def orchestrate(io, manifest, attempt):
    """One monotonic attempt, never reapply or recover after ambiguity. Injectable fixed-fake seam."""
    root = io.root
    session, stage = None, "migrate"
    try:
        io.aoe("migrate")
        stage = "add"
        io.aoe("add", str(root / "work"), "--tool", "pi", "--title", "native-question-diagnostic")
        rows = a.read_json(Path(attempt["app"]) / "profiles/default/sessions.json", 8 * 1024 * 1024, False)
        if len(rows) != 1 or rows[0]["tool"] != "pi" or rows[0].get("command", ""):
            raise ValueError("not a sole managed direct Pi session")
        session = rows[0]["id"]
        initial_deadline = time.monotonic() + manifest["initial_seconds"]
        stage = "start"
        io.aoe("session", "start", session)
        stage = "terminal"
        io.terminal()
        stage = "ready"
        ready = io.marker("ready.json", max(0, initial_deadline - time.monotonic()))
        bootstrap = a.read_json(root / "bootstrap.json")
        if ready.get("binding") != bootstrap["binding"] or ready["binding"]["instance_id"] != session or not ready.get("ready"):
            raise ValueError("native readiness identity differs")
        batch = {"version": 1, "profile": "default", "minutes": 1, "shared_requests": None,
                 "members": [{"session_id": session, "grant": attempt["grant"], "usage_guard": attempt["usage_guard"]}]}
        a.publish(root / "batch.json", batch)
        stage = "preview"
        io.aoe("session", "afk", "batch", "preview", "--manifest", str(root / "batch.json"), "--output", str(root / "preview.json"))
        preview = a.read_json(root / "preview.json")
        content = {key: value for key, value in preview.items() if key != "digest"}
        if preview["digest"] != a.digest(content) or preview["profile"] != "default" or len(preview["members"]) != 1 or preview["version"] != 3 or preview["format"] != "aoe-afk-batch-preview":
            raise ValueError("invalid preview")
        if preview["minutes"] != 1 or preview["shared_requests"] is not None:
            raise ValueError("invalid preview")
        member = preview["members"][0]
        if member["session_id"] != session or member["exclusions"] or member["grant"] != attempt["grant"] or member["usage_guard"] != attempt["usage_guard"]:
            raise ValueError("unqualified preview member")
        expected = preview_approval(manifest, attempt, preview)
        a.publish(root / "preview-approval-required.json", expected, 0o400)
        stage = "preview-approval"
        approval = io.marker("preview-approval.json", manifest["preview_approval_seconds"])
        if a.canonical(approval) != a.canonical(expected):
            raise ValueError("exact preview approval differs")
        # Consume exact apply admission before entering AoE, including any failed publication.
        a.publish(root / "apply-admitted.json", expected, 0o400)
        stage = "apply"
        apply_output = io.aoe("session", "afk", "batch", "apply", "--preview", str(root / "preview.json"), "--digest", preview["digest"])
        a.publish(root / "apply-result.json", json.loads(apply_output))
        stage = "endpoint"
        io.marker("pending.json", 10)
        # Passive reads can wait for the ordinary runtime's own publication, never add a poll RPC.
        deadline = min(io.deadline, time.monotonic() + 5)
        last, captures, capture_bytes = None, 0, 0
        while True:
            try:
                last = io.capture(attempt, preview)
                captures += 1
                capture_bytes += len(a.canonical(last))
                if captures > manifest["capture_attempts"] or capture_bytes > manifest["capture_bytes"]:
                    raise ValueError("independent capture capacity exhausted; prior evidence retained")
                # The endpoint check runs in-process through a separate fixed Node projection command.
                capture_path = root / "capture.json"
                a.publish(capture_path, last)
                endpoint = io.endpoint(last)
                if endpoint.get("endpoint_status") == "qualified":
                    break
                # Keep every attempted capture instead of erasing negative facts.
                capture_path.rename(root / f"capture-unqualified-{time.monotonic_ns()}.json")
            except (FileNotFoundError, BlockingIOError) as error:
                last = {"error": str(error)}
            if time.monotonic() >= deadline:
                raise ValueError(f"endpoint capture inconclusive: {last!r}"[:4096])
            if last and "lease" in last:
                io.wait_capture_change(last, deadline)
            else:
                pause()
        stage = "cancel"
        io.arm()
        if io.marker("wait-armed.json", 5) != {"armed": True}:
            raise ValueError("native completion not armed")
        io.control("cancel")
        if io.marker("run-completed.json", 10) != {"completed": True}:
            raise ValueError("cancelled SDK completion unavailable")
        stage = "off"
        io.aoe("session", "afk", "off", session)
        off_snapshot = a.read_json(Path(attempt["app"]) / "afk-authority-v3/profiles/default/snapshot.json", 32 * 1024 * 1024)
        stored = off_snapshot["sessions"][session]
        if stored["policy"]["enabled"] is not False or any(w["state"] != "ended" for w in stored["book"]["windows"]):
            raise ValueError("explicit Off not stored")
        a.publish(root / "off-authority.json", off_snapshot)
        io.control("off")
        if io.marker("off-seen.json", 5) != {"off": True}:
            raise ValueError("native Off observation unavailable")
        stage = "shutdown"
        io.control("shutdown")
        io.marker("process.json", 10)
        return {"controller": "completed", "session": session}
    except Exception as error:
        a.publish(root / "controller-error.json", {"error": str(error), "session": session, "stage": stage, "classification": "native_inconclusive"})
        # No retry, guard recovery, refund, extra grant or simulated successful cleanup.
        raise


def recv_line(channel, cap=4096):
    data = bytearray()
    while not data.endswith(b"\n"):
        chunk = channel.recv(1)
        if not chunk:
            raise EOFError("private controller channel closed")
        data.extend(chunk)
        if len(data) > cap:
            raise ValueError("private controller frame cap")
    return bytes(data)


def assemble_report(root, manifest, env):
    failure = optional(root / "controller-error.json")
    primary = None
    if failure is not None:
        if isinstance(failure, dict) and isinstance(failure.get("error"), str):
            stage = failure.get("stage", "controller")
            primary = f"{stage}: {failure['error']}"[:4096]
        else:
            primary = "controller: invalid error receipt"
        if absent(root / "capture.json"):
            return {"kind": a.KIND, "classification": "native_inconclusive", "error": primary, "manifest": a.digest(manifest)}
    try:
        output = a.bounded_command([manifest["tools"]["node"]["path"], str(a.HERE / "aoe-afk-native-report.mjs"), str(root)], env, root, 5)
        report = json.loads(output)
    except Exception as error:
        report = {"kind": a.KIND, "classification": "native_inconclusive", "error": primary or str(error)}
    report["manifest"] = a.digest(manifest)
    return report


def retire_terminal_socket(server, path, identity):
    if server.returncode is None:
        raise ValueError("terminal server has not been reaped")
    if server.returncode != 0:
        return False
    try:
        current = path.lstat()
    except FileNotFoundError:
        return True
    if (not stat.S_ISSOCK(current.st_mode) or current.st_uid != os.getuid()
            or current.st_nlink != 1 or (current.st_dev, current.st_ino) != identity):
        return False
    path.unlink()
    return absent(path)


def process_identity(pid, env, cwd):
    if not isinstance(pid, int) or pid <= 1:
        raise ValueError("invalid owned process")
    output = a.bounded_command(["/bin/ps", "-p", str(pid), "-o", "ppid=,lstart="], env, cwd, 2, 4096).decode().strip()
    fields = output.split(maxsplit=1)
    if len(fields) != 2:
        raise ValueError("process identity unavailable")
    return {"pid": pid, "parent": int(fields[0]), "start": fields[1]}


def resume_owner(identity, env, cwd):
    if process_identity(identity["pid"], env, cwd) != identity:
        raise ValueError("cleanup owner changed; refusing historical PID")
    os.kill(identity["pid"], signal.SIGCONT)


def check_pane(hello, server, tmux, env, root):
    pane = hello["pane"]
    if not re.fullmatch(r"%[0-9]+", pane):
        raise ValueError("invalid managed pane")
    output = a.bounded_command([tmux, "-S", env["AOE_TMUX_SOCKET"], "display-message", "-p", "-t", pane,
                               "#{pid}:#{pane_id}:#{pane_pid}"], env, root, 2).decode().strip().split(":")
    if len(output) != 3 or output[:2] != [str(server.pid), pane]:
        raise ValueError("foreign tmux server/pane")
    owner = process_identity(hello["owner"], env, root)
    ancestor = owner
    for _ in range(12):
        if ancestor["pid"] == int(output[2]):
            return owner
        if ancestor["parent"] <= 1:
            break
        ancestor = process_identity(ancestor["parent"], env, root)
    raise ValueError("pane-local owner is outside owned terminal")


def run(manifest, role, ordinal):
    def interrupted(_signal, _frame):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        raise InterruptedError("guardian interrupted; cleanup ownership retained")
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    root = Path(manifest["root"]) / role
    attempt = initialize(root, manifest, role, ordinal)
    env = a.isolated_env(root, manifest)
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(root / "controller.sock"))
    socket_identity = (root / "controller.sock").lstat()
    os.chmod(root / "controller.sock", 0o600)
    listener.listen(1)
    listener.setblocking(False)
    # Foreground server remains our actual child; never signal an installed/default tmux server.
    server = subprocess.Popen([manifest["tools"]["tmux"]["path"], "-S", env["AOE_TMUX_SOCKET"], "-f", "/dev/null", "-D"],
                              env=env, cwd=root / "work", stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, start_new_session=True)
    parent, child = socket.socketpair()
    controller = None
    channel, owner, hello, watchdog = None, None, None, False
    control_buffer = b""
    result = None
    status, guardian_error = None, None
    deadline = time.monotonic() + manifest["attempt_seconds"]
    try:
        await_value(lambda: server.poll() is None and not absent(env["AOE_TMUX_SOCKET"]), time.monotonic() + 5)
        tmux_socket = Path(env["AOE_TMUX_SOCKET"])
        tmux_meta = tmux_socket.lstat()
        if not stat.S_ISSOCK(tmux_meta.st_mode) or tmux_meta.st_uid != os.getuid() or tmux_meta.st_nlink != 1:
            raise ValueError("unsafe terminal socket")
        tmux_identity = (tmux_meta.st_dev, tmux_meta.st_ino)
        controller = os.fork()
        if controller == 0:
            parent.close()
            listener.close()
            try:
                orchestrate(NativeIO(root, manifest, child), manifest, attempt)
                code = 0
            except Exception:
                code = 1
            child.close()
            os._exit(code)
        child.close()
        parent.setblocking(False)
        while True:
            if status is None:
                reaped, value = os.waitpid(controller, os.WNOHANG)
                if reaped:
                    status = value
            if channel is None and not watchdog and select.select([listener], [], [], 0)[0]:
                channel, _ = listener.accept()
                channel.settimeout(3)
                hello = json.loads(recv_line(channel))
                if hello.get("phase") != "before-pi":
                    raise ValueError("missing prelaunch custody barrier")
                owner = check_pane(hello, server, manifest["tools"]["tmux"]["path"], env, root)
                a.publish(root / "terminal.json", {"server": server.pid, "socket": env["AOE_TMUX_SOCKET"], "pane": hello["pane"], "owner": owner})
                channel.sendall(b"launch\n")
                started = json.loads(recv_line(channel))
                if started.get("owner") != owner["pid"] or started.get("pane") != hello["pane"]:
                    raise ValueError("custody identity changed")
                pi = process_identity(started["pi"], env, root)
                if pi["parent"] != owner["pid"]:
                    raise ValueError("Pi is not the custody owner's real child")
                parent.sendall(a.canonical({**started, "server": server.pid}) + b"\n")
                channel.setblocking(False)
            if channel and select.select([parent], [], [], 0)[0]:
                data = parent.recv(1024)
                if data:
                    control_buffer += data
                    if len(control_buffer) > 1024:
                        raise ValueError("controller control cap")
                    while b"\n" in control_buffer:
                        line, control_buffer = control_buffer.split(b"\n", 1)
                        if line not in (b"cancel", b"off", b"shutdown"):
                            raise ValueError("invalid controller frame")
                        channel.sendall(line + b"\n")
                elif status is None:
                    reaped, value = os.waitpid(controller, os.WNOHANG)
                    if reaped:
                        status = value
            result = optional(root / "process.json")
            if result is not None and status is not None:
                break
            if not watchdog and (time.monotonic() >= deadline or (status is not None and result is None)):
                watchdog = True
                a.publish(root / "watchdog.json", {"expired": time.monotonic() >= deadline, "controller_status": status,
                          "cleanup_owner_retained": owner is not None})
                if owner:
                    resume_owner(owner, env, root)
                if channel:
                    channel.close()
                    channel = None
                if status is None:
                    # The controller is our direct child, not the Pi cleanup owner.
                    os.kill(controller, signal.SIGCONT)
                    os.kill(controller, signal.SIGTERM)
                # No forced terminal destruction before the pane owner publishes actual waitpid.
            if server.poll() is not None and result is None:
                raise ValueError("owned tmux server exited before Pi custody receipt")
            if watchdog and owner is None:
                # No Pi was admitted through the guardian barrier, so no native wait handle exists.
                break
            pause()
        clean = result is not None and result["waitpid"] is not None and result["audit_eof"] and result["cleanup"]
        if result is None and owner is None:
            clean = True
        if clean:
            a.bounded_command([manifest["tools"]["tmux"]["path"], "-S", env["AOE_TMUX_SOCKET"], "kill-server"], env, root, 5)
            server.wait(timeout=5)
            cleanup = retire_terminal_socket(server, tmux_socket, tmux_identity)
        else:
            a.publish(root / "terminal-uncertain.json", {"cleanup": False, "owner_retained": owner, "server": server.pid})
            # Retain guardian, terminal and evidence until external inspection. Never kill the cleanup owner.
            while server.poll() is None:
                pause()
            cleanup = False
        listener.close()
        if channel:
            channel.close()
            channel = None
        current_socket = (root / "controller.sock").lstat()
        if (current_socket.st_dev, current_socket.st_ino) == (socket_identity.st_dev, socket_identity.st_ino):
            (root / "controller.sock").unlink()
        else:
            cleanup = False
        cleanup = cleanup and absent(root / "controller.sock")
        if controller and status is None:
            while True:
                try:
                    status = os.waitpid(controller, 0)[1]
                    break
                except InterruptedError:
                    continue
        a.publish(root / "terminal-cleanup.json", {"cleanup": cleanup, "watchdog": watchdog, "controller_status": status})
        report = assemble_report(root, manifest, env)
        a.publish(root / "report.json", report, 0o400)
        print(json.dumps(report))
    except Exception as error:
        guardian_error = str(error)
        a.publish(root / "guardian-error.json", {"error": guardian_error, "classification": "native_inconclusive"})
    finally:
        parent.close()
        child.close()
        listener.close()
        if channel:
            channel.close()
        if controller and status is None:
            reaped, value = os.waitpid(controller, os.WNOHANG)
            if reaped:
                status = value
            else:
                os.kill(controller, signal.SIGCONT)
                os.kill(controller, signal.SIGTERM)
        # If an exception precedes ordinary cleanup, retain the cleanup owner and exact server.
        if server.poll() is None:
            if owner is None:
                # No pre-Pi barrier was released. It is safe to retire only this owned server.
                server.send_signal(signal.SIGCONT)
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
            if owner:
                try:
                    resume_owner(owner, env, root)
                except (ValueError, ProcessLookupError, PermissionError):
                    pass
            if not (root / "guardian-uncertain.json").exists():
                a.publish(root / "guardian-uncertain.json", {"server": server.pid, "owner": owner, "cleanup": False})
            while server.poll() is None:
                receipt = optional(root / "process.json")
                if receipt and receipt["waitpid"] is not None and receipt["audit_eof"] and receipt["cleanup"]:
                    a.bounded_command([manifest["tools"]["tmux"]["path"], "-S", env["AOE_TMUX_SOCKET"], "kill-server"], env, root, 5)
                    break
                pause()
        server.wait()
        if controller and status is None:
            status = os.waitpid(controller, 0)[1]
        if guardian_error and not (root / "report.json").exists():
            a.publish(root / "report.json", {"kind": a.KIND, "manifest": a.digest(manifest), "classification": "native_inconclusive", "error": guardian_error}, 0o400)
            print(json.dumps(a.read_json(root / "report.json")))
