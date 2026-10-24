#!/usr/bin/env python3
"""Private native preparation and explicit later execution. Never imported by the controlled runner."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import re
import shlex
import signal
import socket
import stat
import subprocess
import sys
import time
from types import ModuleType

KIND = "aoe-afk-native-v1"
BASE = "27d66761145d5499e9994eddb4006a93e080420a"
PINS = {"pi": "372da38da1cf6c2912648713c66e3de6a76d030f",
        "question": "424190fa5402a53fd0ec7f128c960dfddebbfbbe"}
HERE = Path(__file__).resolve().parent
LIMIT = 256 * 1024
PYTHON_FLAGS = ("-I", "-S", "-B")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def file_hash(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def private_dir(path):
    path = Path(path)
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise ValueError("symlink directory")
    s = path.stat()
    if not stat.S_ISDIR(s.st_mode) or s.st_uid != os.getuid() or stat.S_IMODE(s.st_mode) != 0o700:
        raise ValueError("owned 0700 directory required")
    return path


def read_bytes(path, limit=LIMIT, private=True):
    path = Path(path)
    if private:
        private_dir(path.parent)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        s = os.fstat(fd)
        if not stat.S_ISREG(s.st_mode) or s.st_nlink != 1 or s.st_uid != os.getuid() or s.st_size > limit:
            raise ValueError("unsafe or oversized file")
        if private and stat.S_IMODE(s.st_mode) not in (0o400, 0o600):
            raise ValueError("private file required")
        data = os.read(fd, limit + 1)
        if len(data) > limit or len(data) != s.st_size:
            raise ValueError("file changed or exceeded bound")
        after = os.fstat(fd)
        if (s.st_mtime_ns, s.st_size, s.st_ino) != (after.st_mtime_ns, after.st_size, after.st_ino):
            raise ValueError("file changed")
        return data
    finally:
        os.close(fd)


def read_json(path, limit=LIMIT, private=True):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON key")
            result[key] = value
        return result
    def invalid_constant(_value):
        raise ValueError("nonfinite JSON value")
    return json.loads(read_bytes(path, limit, private), object_pairs_hook=pairs, parse_constant=invalid_constant)


def publish(path, value, mode=0o600):
    publish_bytes(path, canonical(value) + b"\n", mode)


def publish_bytes(path, data, mode=0o600):
    path = Path(path)
    private_dir(path.parent)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        if os.write(fd, data) != len(data):
            raise ValueError("short write")
        os.fsync(fd)
    finally:
        os.close(fd)
    fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def tree_hash(root, tracked=False):
    root = Path(root).resolve(strict=True)
    if tracked:
        names = subprocess.check_output(["git", "-C", str(root), "ls-files", "-z"]).decode().split("\0")
        paths = [root / name for name in names if name]
    else:
        return dependency_hash(root)
    rows = []
    for path in paths:
        if path.is_symlink():
            target = path.resolve(strict=True)
            rows.append([str(path.relative_to(root)), "link", os.readlink(path)])
            if target.is_file():
                rows.append([str(path.relative_to(root)), "target", file_hash(target)])
        elif path.is_file():
            rows.append([str(path.relative_to(root)), file_hash(path)])
    return digest(rows)


def dependency_hash(root):
    cache, active = {}, set()
    entries, total_bytes = 0, 0

    def visit(path):
        nonlocal entries, total_bytes
        entries += 1
        if entries > 200000 or len(active) > 128:
            raise ValueError("dependency closure entry/depth cap")
        path = path.resolve(strict=True)
        name = str(path)
        if name in active:
            return digest(["cycle", name])
        if name in cache:
            return cache[name]
        if path.is_file():
            total_bytes += path.stat().st_size
            if total_bytes > 2 * 1024 * 1024 * 1024:
                raise ValueError("dependency closure byte cap")
            return file_hash(path)
        if not path.is_dir():
            raise ValueError("non-file dependency")
        active.add(name)
        rows = []
        for child in sorted(path.iterdir()):
            if child.name == ".git":
                continue
            rows.append([child.name, os.readlink(child) if child.is_symlink() else None, visit(child)])
        active.remove(name)
        cache[name] = digest(rows)
        return cache[name]

    return visit(Path(root))


def source_seal(config):
    seals = {}
    receipt = config["binary_receipt"]
    for key, pin in {"aoe": receipt["source_commit"], **PINS}.items():
        root = Path(config["sources"][key]).resolve(strict=True)
        head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
        if head != pin:
            raise ValueError(f"{key} source pin differs")
        dirty = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=no"])
        if dirty:
            raise ValueError(f"{key} tracked source changed")
        seals[key] = {"path": str(root), "head": head, "tree": tree_hash(root, True)}
        if key == "aoe":
            subprocess.run(["git", "-C", str(root), "merge-base", "--is-ancestor", BASE, head], check=True)
            git_tree = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD^{tree}"], text=True).strip()
            if git_tree != receipt["source_tree"] or receipt["binary_sha256"] != config["tools"]["aoe"]["sha256"]:
                raise ValueError("binary receipt source/tree/hash mismatch")
            seals[key]["git_tree"] = git_tree
            seals[key]["baseline"] = BASE
        if key != "aoe":
            dependencies = root / "node_modules"
            if key == "question" and not os.path.lexists(dependencies):
                # The pinned peer-only question package uses Pi's source loader.
                seals[key]["dependencies"] = {"provided_by": "pinned-pi", "closure": seals["pi"]["dependencies"]}
            else:
                seals[key]["dependencies"] = tree_hash(dependencies)
    # Include the adapter's runtime and observer imports.
    seals["adapter"] = {str(p): file_hash(p) for p in sorted(HERE.glob("aoe-afk*")) if p.is_file()}
    return seals


def verify_tools(config):
    result = {}
    for key in ["node", "python", "tmux", "aoe", "fd", "rg", "loader"]:
        pin = config["tools"][key]
        path = str(Path(pin["path"]).resolve(strict=True))
        if file_hash(path) != pin["sha256"]:
            raise ValueError(f"{key} hash differs")
        result[key] = {"path": path, "sha256": pin["sha256"]}
    expected = str(Path(config["sources"]["pi"]).resolve() / "node_modules/tsx/dist/loader.mjs")
    if result["loader"]["path"] != expected:
        raise ValueError("loader must be the source checkout's scoped tsx")
    # Metadata only, no Pi or configured host probe.
    version = subprocess.check_output([result["node"]["path"], "--version"], env={"PATH": "/usr/bin:/bin"}, text=True).strip()
    if version != config["node_version"]:
        raise ValueError("Node version differs")
    return result


def proposal():
    model = {"id": "native-question", "name": "synthetic native diagnostic", "provider": "aoe-native-fixed",
             "api": "aoe-native-fixed", "baseUrl": "http://invalid.invalid", "reasoning": False, "input": ["text"],
             "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}, "contextWindow": 100000, "maxTokens": 1000}
    usage = {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2,
             "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}}
    guard = {"version": 1, "basis": "observed-native-entries", "tokens": None,
             "usd": {"currency": "USD", "basis": "operator-fixed-rates", "limit_nano": 100,
                     "quotes": [{"model": {"provider": model["provider"], "model": model["id"]},
                                 "input": 1000000, "output": 1000000, "cache_read": 1000000,
                                 "cache_write": 1000000, "cache_write_1h": 1000000}]}}
    return {"model": model, "ordinary_usage": usage, "usage_guard": guard,
            "pricing_note": "Proposed artificial nanoUSD/million-token rates, not historical C pricing or billing proof. Ordinary response estimates 2 nanoUSD. Cancellation leaves an error_or_abort gap, never financial finality.",
            "question": {"question": "Leave this disposable diagnostic question unanswered."},
            "grant": {"version": 4, "task": "native question diagnostic", "scope": "scratch.txt only; no action branch; do not answer the question",
                      "files": [{"path": "scratch.txt", "capability": "create"}], "requests": 2, "question_deferrals": 1,
                      "settlement_nudges": 0, "assurance": "stock-sdk-main-loop-reservations"}}


def prepare(config_path, destination):
    config = read_json(config_path)
    tools, seals = verify_tools(config), source_seal(config)
    receipt = config["binary_receipt"]
    if receipt["binary"] != tools["aoe"]["path"] or receipt.get("installed") is not False:
        raise ValueError("fresh uninstalled binary receipt required")
    if receipt["profile"] not in ("debug", "release", "dev-release"):
        raise ValueError("unknown build profile")
    expected_app = "agent-of-empires-dev" if config["binary_receipt"]["profile"] == "debug" else "agent-of-empires"
    if config["app_name"] != expected_app:
        raise ValueError("explicit build namespace differs from binary receipt")
    if json.loads((Path(seals["pi"]["path"]) / "packages/coding-agent/package.json").read_text())["version"] != "0.87.1":
        raise ValueError("unsupported source SDK version")
    root = Path(destination).resolve()
    if len(os.fsencode(root / "observed/controller.sock")) > 100:
        raise ValueError("use a short private root for Unix socket paths")
    root.mkdir(mode=0o700)
    private_dir(root)
    manifest = {"kind": KIND, "version": 1, "root": str(root), "config": config, "tools": tools, "sources": seals,
                "experiment": proposal(), "minutes": 1, "initial_seconds": 30,
                "preview_approval_seconds": config.get("preview_approval_seconds", 30), "attempt_seconds": 120,
                "capture_attempts": 10, "capture_bytes": 64 * 1024 * 1024,
                "roles": ["baseline", "observed"], "authority": "explicit later exact-digest invocation only",
                "qualification": "unexecuted; integrity is not same-UID authentication"}
    if not isinstance(manifest["preview_approval_seconds"], int) or not 1 <= manifest["preview_approval_seconds"] <= 30:
        raise ValueError("preview approval wait must be 1..30 seconds")
    publish(root / "manifest.json", manifest, 0o400)
    script = f"#!/bin/sh\nexec {shlex.quote(tools['python']['path'])} {' '.join(PYTHON_FLAGS)} {shlex.quote(str(HERE / 'aoe-afk-native.py'))} pane {shlex.quote(str(root / 'manifest.json'))} \"$@\"\n"
    launcher = root / "pi"
    with open(launcher, "x") as f:
        f.write(script)
        f.flush()
        os.fsync(f.fileno())
    launcher.chmod(0o500)
    runner = root / "run"
    with open(runner, "x") as f:
        f.write(f"#!/bin/sh\nexec {shlex.quote(tools['python']['path'])} {' '.join(PYTHON_FLAGS)} {shlex.quote(str(HERE / 'aoe-afk-native.py'))} run {shlex.quote(str(root / 'manifest.json'))} \"$@\"\n")
        f.flush()
        os.fsync(f.fileno())
    runner.chmod(0o500)
    publish(root / "seal.json", {"manifest": digest(manifest), "launcher": file_hash(launcher), "runner": file_hash(runner)}, 0o400)
    return {"manifest": digest(manifest), "root": str(root), "status": "prepared_not_authorized_or_qualified"}


def admit(manifest_path, approval_path, role, ordinal):
    manifest = read_json(manifest_path)
    if read_bytes(manifest_path) != canonical(manifest) + b"\n":
        raise ValueError("changed manifest bytes")
    if manifest.get("kind") != KIND or type(manifest.get("version")) is not int or manifest["version"] != 1 or manifest.get("roles") != ["baseline", "observed"]:
        raise ValueError("invalid native manifest")
    root = private_dir(manifest["root"])
    if Path(manifest_path).resolve() != root / "manifest.json":
        raise ValueError("manifest outside prepared root")
    for name in ["pi", "run"]:
        s = (root / name).lstat()
        if not stat.S_ISREG(s.st_mode) or stat.S_IMODE(s.st_mode) != 0o500 or s.st_uid != os.getuid() or s.st_nlink != 1:
            raise ValueError("private immutable launcher required")
    seal = read_json(root / "seal.json")
    approval = read_json(approval_path)
    expected = {"kind": KIND, "manifest": digest(manifest), "role": role, "ordinal": ordinal,
                "approve_native_execution": True, "approve_synthetic_pricing": True}
    if canonical(approval) != canonical(expected) or role not in ("baseline", "observed") or type(ordinal) is not int or ordinal != (1 if role == "baseline" else 2):
        raise ValueError("exact manifest/role/ordinal approval required")
    if seal != {"manifest": digest(manifest), "launcher": file_hash(root / "pi"), "runner": file_hash(root / "run")}:
        raise ValueError("changed manifest or launcher")
    if verify_tools(manifest["config"]) != manifest["tools"] or source_seal(manifest["config"]) != manifest["sources"]:
        raise ValueError("changed pinned input")
    if role == "observed":
        baseline = read_json(root / "baseline" / "report.json")
        if baseline.get("classification") != "native_endpoint_only" or baseline.get("manifest") != digest(manifest):
            raise ValueError("baseline failed or missing; new explicit decision required")
    # O_EXCL and fsync precede every executable native operation. Failed starts consume this slot.
    publish(root / f"admitted-{ordinal}.json", expected, 0o400)
    return manifest


def bounded_command(argv, env, cwd, timeout=10, limit=LIMIT):
    selector, failure, code = None, None, None
    output, eof, owned = bytearray(), False, True

    def remember(error):
        nonlocal failure
        if failure is None:
            failure = error
        elif failure is not error:
            failure.add_note(f"command cleanup: {error}")

    def exited():
        nonlocal owned
        try:
            if child.returncode is not None:
                raise ChildProcessError("command reaped outside cleanup owner")
            return os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
        except ChildProcessError:
            owned = False
            raise

    child = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, start_new_session=True)
    try:
        deadline = time.monotonic() + timeout
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        while True:
            if exited() and eof:
                break
            if time.monotonic() >= deadline:
                raise ValueError("owned command deadline")
            for key, _ in selector.select(min(0.02, max(0, deadline - time.monotonic()))):
                data = os.read(key.fd, min(4096, limit + 1 - len(output)))
                if not data:
                    eof = True
                    selector.unregister(key.fileobj)
                else:
                    output.extend(data)
                    if len(output) > limit:
                        raise ValueError("owned command output exceeds physical cap")
    except BaseException as error:
        remember(error)
    finally:
        try:
            if selector is not None:
                selector.close()
        except BaseException as error:
            remember(error)
        finally:
            try:
                child.stdout.close()
            except BaseException as error:
                remember(error)
            finally:
                try:
                    # WNOWAIT reserves the owned PID through every possible group signal.
                    if owned and (failure is not None or not eof or not exited()):
                        exited()
                        os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except BaseException as error:
                    remember(error)
                finally:
                    if owned:
                        while True:
                            try:
                                code = child.wait()
                                break
                            except BaseException as error:
                                remember(error)
    if failure is not None:
        raise failure
    if code != 0:
        raise ValueError(f"bounded command failed ({code}): {bytes(output[:4096])!r}")
    return bytes(output)


def isolated_env(root, manifest):
    root = Path(root)
    env = {"PATH": str(root / "bin") + ":/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(root / "home"),
           "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
           "XDG_CACHE_HOME": str(root / "cache"), "XDG_STATE_HOME": str(root / "state"),
           "PI_CODING_AGENT_DIR": str(root / "agent"), "PI_CODING_AGENT_SESSION_DIR": str(root / "sessions"),
           "TMPDIR": str(root / "tmp"),
           "SHELL": "/bin/sh", "TERM": "xterm-256color", "LANG": "en_US.UTF-8",
           "PI_OFFLINE": "1", "PI_TELEMETRY": "0", "PI_SKIP_VERSION_CHECK": "1", "DO_NOT_TRACK": "1",
           "AOE_TMUX_SOCKET": str(root / "tmux.sock"), "AOE_NATIVE_ATTEMPT": str(root),
           "TSX_TSCONFIG_PATH": str(Path(manifest["sources"]["pi"]["path"]) / "tsconfig.json")}
    return env


def validate_forwarded(argv, manifest, attempt):
    if len(argv) != 8 or argv[0] != "-e" or argv[2] != "--session-id" or argv[4] != "-e" or argv[6] != "--aoe-afk-binding":
        raise ValueError("only exact fresh managed Pi argv accepted")
    if len(argv[7].encode()) > 16 * 1024:
        raise ValueError("oversized managed bootstrap")
    bootstrap = json.loads(argv[7])
    binding = bootstrap["binding"]
    if set(binding) != {"instance_id", "profile", "native_id", "launch_id"}:
        raise ValueError("invalid binding shape")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", binding["instance_id"]):
        raise ValueError("invalid instance identity")
    for field in ["native_id", "launch_id"]:
        if not re.fullmatch(r"[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}", binding[field]):
            raise ValueError("invalid native/launch UUID")
    app = Path(attempt["app"])
    if bootstrap["version"] != 6 or bootstrap["app_dir"] != str(app) or bootstrap["aoe_bin"] != manifest["tools"]["aoe"]["path"]:
        raise ValueError("foreign bootstrap binary/app")
    if binding["native_id"] != argv[3] or binding["profile"] != "default":
        raise ValueError("foreign native binding")
    identity = app / "agent-extensions" / "pi-aoe-session-id.js"
    identity_source = Path(manifest["sources"]["aoe"]["path"]) / "assets/session/aoe-session-id.js"
    if argv[1] != str(identity) or identity.is_symlink() or file_hash(identity) != file_hash(identity_source):
        raise ValueError("not exact generated session identity extension")
    expected = app / "agent-extensions" / "aoe-afk-runtime.mjs"
    if argv[5] != str(expected):
        raise ValueError("not exact generated AFK extension")
    for name in ["aoe-afk.mjs", "aoe-afk-runtime.mjs", "aoe-afk-wake.mjs", "aoe-afk-usage.mjs", "aoe-afk-usage-guard.mjs", "aoe-afk-questions.mjs"]:
        path = app / "agent-extensions" / name
        if path.is_symlink() or file_hash(path) != file_hash(HERE / name):
            raise ValueError("embedded runtime differs from reviewed AoE source")
    registry = read_json(app / "profiles/default/sessions.json", 8 * 1024 * 1024, False)
    if not isinstance(registry, list) or len(registry) != 1:
        raise ValueError("exactly one managed session required")
    rows = registry
    row = next(r for r in rows if r["id"] == binding["instance_id"])
    if not row.get("active_execution"):
        raise KeyError("managed active execution not yet published")
    if row["tool"] != "pi" or row.get("command", "") != "" or row["agent_session_id"] != binding["native_id"] or row["active_execution"]["launch_id"] != binding["launch_id"]:
        raise ValueError("managed registry identity differs")
    execution = row["active_execution"]["binding"]
    root = app.parent.parent
    if execution["agent"] != "pi" or execution["stores"] != [str(root / "sessions")] or execution["cwd"] != str(root / "work") or execution["filesystem"] != "host":
        raise ValueError("native execution store/workspace differs")
    return bootstrap


def valid_audit(value):
    keys = {"kind", "version", "manifest", "role", "ordinal", "waitArmed", "cancelled", "consumed", "settled", "off", "shutdown", "writersClosed", "fault", "trace", "probes", "publication"}
    if not isinstance(value, dict) or set(value) != keys or value["kind"] != KIND or type(value["version"]) is not int or value["version"] != 1:
        return False
    if not isinstance(value["manifest"], str) or not re.fullmatch(r"[0-9a-f]{64}", value["manifest"]):
        return False
    if value["role"] not in ("baseline", "observed") or type(value["ordinal"]) is not int or value["ordinal"] != (1 if value["role"] == "baseline" else 2):
        return False
    if any(type(value[k]) is not bool for k in ["waitArmed", "cancelled", "consumed", "settled", "off", "shutdown", "writersClosed", "fault"]):
        return False
    publication = value["publication"]
    if (not isinstance(publication, dict) or set(publication) != {"uncertain", "truncated", "receipts"}
            or type(publication["uncertain"]) is not bool or type(publication["truncated"]) is not bool
            or not isinstance(publication["receipts"], list) or len(publication["receipts"]) > 16):
        return False
    if publication["uncertain"]:
        if not publication["truncated"] and not publication["receipts"]:
            return False
    elif publication["truncated"] or publication["receipts"]:
        return False
    total = 0
    for row in publication["receipts"]:
        if (not isinstance(row, dict) or set(row) != {"binding", "generation", "request", "op", "data_commit", "warnings"}
                or row["data_commit"] is not True or type(row["request"]) is not int or not 1 <= row["request"] <= 65535
                or row["op"] not in ("poll", "ack", "runtime_ack", "runtime", "question_recover", "usage_recover", "other")
                or not isinstance(row["generation"], str) or not re.fullmatch(r"[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}", row["generation"])
                or not isinstance(row["warnings"], list) or not 1 <= len(row["warnings"]) <= 8
                or any(not isinstance(w, str) or not 1 <= len(w) <= 256 for w in row["warnings"])):
            return False
        binding = row["binding"]
        if (not isinstance(binding, dict) or set(binding) != {"profile", "instance_id", "native_id", "launch_id"}
                or any(not isinstance(v, str) or not 1 <= len(v) <= 256 or re.search(r"[\x00-\x1f\x7f]", v) for v in binding.values())
                or any(not re.fullmatch(r"[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}", binding[k]) for k in ["native_id", "launch_id"])):
            return False
        total += len(canonical(row))
    if total > 16 * 1024:
        return False
    for name in ["trace", "probes"]:
        writer = value[name]
        if value["role"] == "baseline":
            if writer is not None:
                return False
        elif (not isinstance(writer, dict) or set(writer) != {"invalid", "truncated", "events", "bytes"}
              or type(writer["invalid"]) is not bool or type(writer["truncated"]) is not bool
              or type(writer["events"]) is not int or not 0 <= writer["events"] <= 1024
              or type(writer["bytes"]) is not int or not 0 <= writer["bytes"] <= LIMIT):
            return False
    return True


def custody(argv, env, channel, timeout=120, terminal=True, evidence_root=None):
    """Keep the actual Pi parent alive through handshake failures and waitpid."""
    child, selector = None, None
    descriptors = set()
    owned, reaped, exited = False, False, False
    audit, control = b"", b""
    eof, control_eof, escalated, lost = False, False, False, False
    terminating, killed = None, False
    signal_errors, io_errors = [], []
    descriptor_error = False

    def lose(error):
        nonlocal lost
        lost = True
        if len(io_errors) < 16:
            io_errors.append(str(error)[:256])

    def close_fd(fd):
        nonlocal descriptor_error
        if fd in descriptors:
            try:
                os.close(fd)
            except OSError as error:
                descriptor_error = True
                lose(error)
            descriptors.remove(fd)

    def observe_exit():
        nonlocal owned, exited
        if not owned:
            return exited
        try:
            if child.returncode is not None:
                raise ChildProcessError("child was reaped outside custody")
            # Preserve the owned PID until all group escalation is finished.
            exited = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
        except ChildProcessError as error:
            owned = False
            lose(error)
        except OSError as error:
            lose(error)
        return exited

    def send_group(sig):
        observe_exit()
        if not owned:
            return
        try:
            os.killpg(child.pid, sig)
        except ProcessLookupError:
            pass
        except OSError as error:
            signal_errors.append(str(error))

    def wait_child():
        nonlocal owned, reaped
        if not owned:
            return None
        try:
            code = child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            signal_errors.append("Pi waitpid deadline; cleanup owner retained")
            if evidence_root:
                try:
                    publish(Path(evidence_root) / "custody-uncertain.json", {"pi": child.pid, "owner": os.getpid(), "signal_errors": signal_errors})
                except Exception as error:
                    lose(error)
            code = child.wait()
        owned, reaped = False, True
        return code

    try:
        if terminal:
            if not os.isatty(0) or not os.isatty(1):
                raise ValueError("managed terminal required")
            signal.signal(signal.SIGTTOU, signal.SIG_IGN)
        audit_r, audit_w = os.pipe()
        descriptors.update((audit_r, audit_w))
        control_r, control_w = os.pipe()
        descriptors.update((control_r, control_w))
        env = {**env, "AOE_NATIVE_AUDIT_FD": str(audit_w), "AOE_NATIVE_CONTROL_FD": str(control_r)}
        child = subprocess.Popen(argv, env=env, pass_fds=(audit_w, control_r), process_group=0)
        owned = True
        # Everything after successful Popen is inside the custody/finally boundary.
        try:
            close_fd(audit_w)
            close_fd(control_r)
            if terminal:
                os.tcsetpgrp(0, child.pid)
            selector = selectors.DefaultSelector()
            selector.register(audit_r, selectors.EVENT_READ, "audit")
            selector.register(channel, selectors.EVENT_READ, "control")
            channel.sendall(canonical({"owner": os.getpid(), "pi": child.pid, "pane": env.get("TMUX_PANE")}) + b"\n")
        except Exception as error:
            lose(error)
        # Child-only ends must close even when terminal/selector/handshake setup fails.
        close_fd(audit_w)
        close_fd(control_r)
        deadline = time.monotonic() + timeout
        while True:
            now = time.monotonic()
            if (lost or now >= deadline) and terminating is None:
                escalated, terminating = True, now
                send_group(signal.SIGCONT)
                send_group(signal.SIGTERM)
            if terminating is not None and now - terminating > 1 and not killed:
                escalated, killed = True, True
                send_group(signal.SIGKILL)
            try:
                if selector is None:
                    with selectors.DefaultSelector() as waiter:
                        waiter.select(0.02)
                    selected = []
                else:
                    selected = selector.select(0.02)
                for key, _ in selected:
                    data = os.read(key.fd, 4096)
                    if key.data == "audit":
                        if not data:
                            eof = True
                            selector.unregister(audit_r)
                        elif len(audit) + len(data) <= LIMIT:
                            audit += data
                        else:
                            lose("audit byte cap exceeded")
                    elif not data:
                        lose("controller EOF")
                        selector.unregister(channel)
                    elif not lost:
                        control += data
                        if len(control) > 1024:
                            lose("control byte cap exceeded")
                            control = b""
                        while b"\n" in control:
                            line, control = control.split(b"\n", 1)
                            if line not in (b"cancel", b"off", b"shutdown"):
                                lose("invalid native control")
                                break
                            try:
                                os.write(control_w, line + b"\n")
                            except BrokenPipeError as error:
                                control_eof = True
                                lose(error)
            except Exception as error:
                lose(error)
            if observe_exit():
                if terminating is None:
                    terminating = now
                if eof:
                    break
            if not owned or (terminating is not None and now - terminating > 3):
                break
        if owned and not exited and not killed:
            send_group(signal.SIGKILL)
        code = wait_child()
        try:
            os.write(control_w, b"shutdown\n")
        except BrokenPipeError:
            control_eof = True
        except OSError as error:
            lose(error)
        group_absent = False
        if reaped:
            try:
                # Observation only. This query never authorizes a post-reap signal.
                os.killpg(child.pid, 0)
            except ProcessLookupError:
                group_absent = True
            except OSError as error:
                signal_errors.append(str(error))
        if terminal:
            try:
                os.tcsetpgrp(0, os.getpgrp())
            except OSError as error:
                lose(error)
        if selector is not None:
            try:
                selector.close()
            except OSError as error:
                descriptor_error = True
                lose(error)
            selector = None
        for fd in list(descriptors):
            close_fd(fd)
        valid, envelope = False, None
        try:
            if audit.count(b"\n") == 1 and audit.endswith(b"\n"):
                envelope = json.loads(audit)
                valid = valid_audit(envelope)
        except (ValueError, AttributeError):
            pass
        return {"waitpid": code, "audit_eof": eof, "control_eof": control_eof, "audit_valid": valid,
                "audit": envelope, "audit_raw": audit.decode("utf8", "replace"), "cleanup": group_absent and not signal_errors and not descriptor_error,
                "escalated": escalated, "controller_lost": lost, "signal_errors": signal_errors, "io_errors": io_errors}
    finally:
        # An unexpected exception must not release a still-waitable direct child.
        try:
            if owned:
                send_group(signal.SIGKILL)
                wait_child()
        finally:
            try:
                if selector is not None:
                    selector.close()
            finally:
                for fd in list(descriptors):
                    close_fd(fd)


def pane(manifest_path, argv):
    # The fixed capability shim never loads Pi, extensions or user settings.
    if argv == ["--help"]:
        print("pi source 0.87.1 native diagnostic: --session-id <id> --extension <path> -e <path>")
        return
    if argv == ["--version"]:
        print("0.87.1")
        return
    manifest = read_json(manifest_path)
    root = private_dir(os.environ["AOE_NATIVE_ATTEMPT"])
    attempt = read_json(root / "attempt.json")
    admission = read_json(Path(manifest["root"]) / f"admitted-{attempt['ordinal']}.json")
    if canonical(admission) != canonical({"kind": KIND, "manifest": digest(manifest), "role": attempt["role"], "ordinal": attempt["ordinal"],
                                         "approve_native_execution": True, "approve_synthetic_pricing": True}):
        raise ValueError("unadmitted native invocation")
    if attempt["manifest"] != digest(manifest):
        raise ValueError("attempt manifest mismatch")
    deadline = time.monotonic() + manifest["initial_seconds"]
    while True:
        try:
            bootstrap = validate_forwarded(argv, manifest, attempt)
            break
        except (KeyError, StopIteration, FileNotFoundError):
            if time.monotonic() >= deadline:
                raise ValueError("managed registry publication deadline")
            with selectors.DefaultSelector() as waiter:
                waiter.select(0.01)
    publish(root / "bootstrap.json", bootstrap)
    env = isolated_env(root, manifest)
    env["TMUX_PANE"] = os.environ.get("TMUX_PANE", "")
    source = Path(manifest["sources"]["pi"]["path"])
    args = [manifest["tools"]["node"]["path"], "--import", manifest["tools"]["loader"]["path"],
            str(source / "packages/coding-agent/src/cli.ts"), "--session-id", bootstrap["binding"]["native_id"], "--session-dir", str(root / "sessions"),
            "--offline", "--no-extensions", "--no-skills", "--no-prompt-templates", "--no-themes", "--no-context-files", "--no-approve",
            "--tools", "ask_user,aoe_afk_read,aoe_afk_record,aoe_afk_apply,aoe_afk_checkpoint",
            "-e", str(HERE / "aoe-afk-native-entry.mjs"), "-e", str(Path(manifest["sources"]["question"]["path"]) / "index.ts"),
            "--aoe-afk-binding", canonical(bootstrap).decode("utf8"), "--provider", attempt["model"]["provider"], "--model", attempt["model"]["id"],
            "Open the fixed disposable diagnostic question."]
    with socket.socket(socket.AF_UNIX) as channel:
        channel.connect(str(root / "controller.sock"))
        channel.sendall(canonical({"owner": os.getpid(), "pane": env["TMUX_PANE"], "phase": "before-pi"}) + b"\n")
        channel.settimeout(5)
        if channel.recv(16) != b"launch\n":
            raise ValueError("terminal owner did not admit Pi launch")
        channel.settimeout(None)
        result = custody(args, env, channel, manifest["attempt_seconds"], evidence_root=root)
        publish(root / "process.json", {**result, "manifest": attempt["manifest"], "role": attempt["role"], "ordinal": attempt["ordinal"]})
        try:
            channel.sendall(b"reaped\n")
        except BrokenPipeError:
            pass


def load_controller():
    path = HERE / "aoe-afk-native-controller.py"
    module = ModuleType("native_controller")
    module.__file__ = str(path)
    exec(compile(read_bytes(path, 128 * 1024, private=False), str(path), "exec"), module.__dict__)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare", help="seal explicit source/tool pins without launching Pi or AoE")
    prep.add_argument("config")
    prep.add_argument("destination")
    run = commands.add_parser("run", help="consume one exact approved attempt; requires a second exact-preview approval before apply")
    run.add_argument("manifest")
    run.add_argument("--approval", required=True)
    run.add_argument("--role", choices=["baseline", "observed"], required=True)
    run.add_argument("--ordinal", type=int, required=True)
    internal = commands.add_parser("pane", help=argparse.SUPPRESS)
    internal.add_argument("manifest")
    internal.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command == "prepare":
        print(json.dumps(prepare(args.config, args.destination)))
    elif args.command == "pane":
        pane(args.manifest, args.argv)
    elif args.command == "run":
        manifest = admit(args.manifest, args.approval, args.role, args.ordinal)
        load_controller().run(manifest, args.role, args.ordinal)


if __name__ == "__main__":
    os.umask(0o077)
    main()
