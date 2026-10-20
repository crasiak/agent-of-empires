"""Only fixed fake OS children. This suite never loads Pi, tmux, AoE or a provider."""
import importlib.util
import errno
import fcntl
import termios
import json
import marshal
import struct
import os
from pathlib import Path
import pty
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

PATH = Path(__file__).with_name("aoe-afk-native.py")
spec = importlib.util.spec_from_file_location("native_adapter", PATH)
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)

FAKE = r'''
import json, os, signal, sys
fd = int(os.environ['AOE_NATIVE_AUDIT_FD'])
control = int(os.environ['AOE_NATIVE_CONTROL_FD'])
case = sys.argv[1]
if case == 'hang':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
if case == 'tty':
    assert os.isatty(0) and os.isatty(1)
    assert os.tcgetpgrp(0) == os.getpgrp()
os.write(1, b'fake-ready\n')
if case == 'hang':
    while True: signal.pause()
if case == 'holder':
    pid = os.fork()
    if pid == 0:
        os.close(control)
        while True: signal.pause()
else:
    line = b''
    while not line.endswith(b'\n'):
        chunk = os.read(control, 1)
        if not chunk: break
        line += chunk
if case == 'malformed': os.write(fd, b'{broken}\n')
elif case == 'overlong':
    try:
        for _ in range(100): os.write(fd, b'x' * 4096)
    except BrokenPipeError: pass
else: os.write(fd, json.dumps({'kind':'aoe-afk-native-v1','version':1,'manifest':'f'*64,'role':'baseline','ordinal':1,
    'waitArmed':False,'cancelled':False,'consumed':False,'settled':False,'off':False,'shutdown':False,'writersClosed':True,'fault':False,'trace':None,'probes':None,'publication':{'uncertain':False,'truncated':False,'receipts':[]}}).encode()+b'\n')
os.close(fd)
os.close(control)
'''
OWNER = r'''
import importlib.util, json, os, socket, sys
spec=importlib.util.spec_from_file_location('adapter',sys.argv[1]); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
s=socket.socket(fileno=int(sys.argv[2]))
r=m.custody([sys.executable,'-c',sys.argv[3],sys.argv[4]],dict(os.environ),s,timeout=float(sys.argv[5]),terminal=sys.argv[4]=='tty')
with open(sys.argv[6],'w') as f: json.dump(r,f)
'''


RECOVER = r'''
import importlib.util, json, os, pathlib, sys, time
spec=importlib.util.spec_from_file_location('controller',sys.argv[1]); c=importlib.util.module_from_spec(spec); spec.loader.exec_module(c)
pid=int(sys.argv[2]); env={'PATH':'/usr/bin:/bin'}; cwd=pathlib.Path.cwd()
identity=c.process_identity(pid,env,cwd)
end=time.monotonic()+3
while True:
    state=c.a.bounded_command(['/bin/ps','-p',str(pid),'-o','state='],env,cwd,1,4096).decode().strip()
    if state.startswith('T'): break
    if time.monotonic()>=end: raise RuntimeError('owner never entered observed stopped state')
    c.pause()
c.resume_owner(identity,env,cwd)
print(json.dumps({'observed_stopped':True,'resumed':True}))
'''


class ProcessTests(unittest.TestCase):
    def run_case(self, case, disconnect=False, stop_owner=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            left, right = socket.socketpair()
            master, slave = pty.openpty()
            result_path = root / "process.json"
            def controlling_terminal():
                os.setsid()
                fcntl.ioctl(0, termios.TIOCSCTTY, 0)
            owner = subprocess.Popen([sys.executable, "-c", OWNER, str(PATH), str(right.fileno()), FAKE, case, "1", str(result_path)],
                                     pass_fds=(right.fileno(),), stdin=slave, stdout=slave, stderr=subprocess.PIPE,
                                     preexec_fn=controlling_terminal if case == "tty" else None)
            right.close()
            os.close(slave)
            try:
                left.settimeout(3)
                data = b""
                while b"\n" not in data:
                    data += left.recv(4096)
                hello = json.loads(data)
                self.assertEqual(hello["owner"], owner.pid)
                # Establish actual child progress, not a sleep-based assumption.
                terminal = b""
                end = time.monotonic() + 3
                while b"fake-ready" not in terminal:
                    if not select.select([master], [], [], max(0, end - time.monotonic()))[0]:
                        self.fail("fake child did not acknowledge readiness")
                    terminal += os.read(master, 4096)
                if stop_owner:
                    os.kill(owner.pid, signal.SIGSTOP)
                    recovery = subprocess.run([sys.executable, "-c", RECOVER, str(PATH.with_name("aoe-afk-native-controller.py")), str(owner.pid)],
                                              capture_output=True, timeout=5)
                    self.assertEqual(recovery.returncode, 0, recovery.stderr.decode())
                    self.assertEqual(json.loads(recovery.stdout), {"observed_stopped": True, "resumed": True})
                if disconnect:
                    release_r, release_w = os.pipe()
                    controller = subprocess.Popen([sys.executable, "-c", "import os,sys; os.read(int(sys.argv[2]),1); os._exit(0)",
                                                   str(left.fileno()), str(release_r)], pass_fds=(left.fileno(), release_r))
                    os.close(release_r)
                    left.close()
                    os.write(release_w, b"exit")
                    os.close(release_w)
                    self.assertEqual(controller.wait(timeout=3), 0)
                elif case != "hang":
                    left.sendall(b"shutdown\n")
                _, stderr = owner.communicate(timeout=8)
                self.assertEqual(owner.returncode, 0, stderr.decode())
                result = json.loads(result_path.read_text())
                self.assertIsInstance(result["waitpid"], int)
                self.assertTrue(result["audit_eof"])
                if case in ("malformed", "overlong"):
                    self.assertFalse(result["audit_valid"])
                elif case != "hang":
                    self.assertTrue(result["audit_valid"])
                if disconnect or case in ("hang", "overlong"):
                    self.assertTrue(result["escalated"])
                if case == "hang":
                    self.assertLess(result["waitpid"], 0)
                self.assertTrue(result["control_eof"])
                return result
            finally:
                left.close()
                os.close(master)
                if owner.poll() is None:
                    os.kill(owner.pid, signal.SIGCONT)
                    owner.kill()
                    owner.wait()

    def test_fixed_children_terminal_audit_eof_waitpid(self):
        for case in ("tty", "malformed", "overlong", "holder", "hang"):
            with self.subTest(case=case):
                result = self.run_case(case)
                if case == "tty":
                    self.assertEqual(result["waitpid"], 0)
                    self.assertTrue(result["cleanup"])
                    self.assertFalse(result["escalated"])

    def test_postspawn_channel_and_io_failures_reap_child_and_close_descriptors(self):
        for failure in ["before-hello", "postspawn-io", "inherited-holder"]:
            with self.subTest(failure=failure):
                left, right = socket.socketpair()
                created, children, signals = [], [], []
                real_pipe, real_popen = adapter.os.pipe, adapter.subprocess.Popen
                real_selector, real_killpg = adapter.selectors.DefaultSelector, adapter.os.killpg
                reaped = False

                def pipe():
                    pair = real_pipe(); created.extend(pair); return pair

                def popen(*args, **kwargs):
                    nonlocal reaped
                    child = real_popen(*args, **kwargs); children.append(child)
                    real_wait, real_poll = child.wait, child.poll
                    def wait(*args, **kwargs):
                        nonlocal reaped
                        code = real_wait(*args, **kwargs); reaped = True; return code
                    def poll(*args, **kwargs):
                        nonlocal reaped
                        code = real_poll(*args, **kwargs)
                        if code is not None: reaped = True
                        return code
                    child.wait, child.poll = wait, poll
                    return child

                def killpg(pid, sig):
                    self.assertFalse(reaped and sig != 0, "mutating group signal after reap")
                    signals.append(sig)
                    return real_killpg(pid, sig)

                class FaultSelector:
                    def __init__(self): self.inner, self.failed = real_selector(), False
                    def register(self, *args): return self.inner.register(*args)
                    def unregister(self, *args): return self.inner.unregister(*args)
                    def close(self): return self.inner.close()
                    def select(self, *args):
                        if failure == "postspawn-io" and not self.failed:
                            self.failed = True
                            raise OSError(errno.EIO, "fixed postspawn I/O fault")
                        return self.inner.select(*args)

                if failure == "before-hello": left.close()
                try:
                    with patch.object(adapter.os, "pipe", side_effect=pipe), patch.object(adapter.subprocess, "Popen", side_effect=popen), \
                         patch.object(adapter.selectors, "DefaultSelector", FaultSelector), patch.object(adapter.os, "killpg", side_effect=killpg):
                        code = "import signal; signal.pause()"
                        if failure == "inherited-holder":
                            code = "import os,signal; pid=os.fork(); os.close(int(os.environ['AOE_NATIVE_CONTROL_FD'])); signal.pause() if pid==0 else os._exit(0)"
                        result = adapter.custody([sys.executable, "-c", code], dict(os.environ), right, timeout=3, terminal=False)
                    self.assertEqual(len(children), 1)
                    if failure == "inherited-holder":
                        self.assertEqual(result["waitpid"], 0)
                        self.assertIn(signal.SIGKILL, signals)
                    else:
                        self.assertLess(result["waitpid"], 0)
                    self.assertEqual(result["waitpid"], children[0].returncode)
                    with self.assertRaises(ChildProcessError): os.waitpid(children[0].pid, os.WNOHANG)
                    self.assertEqual(result["controller_lost"], failure != "inherited-holder")
                    self.assertTrue(result["escalated"])
                    self.assertTrue(result["audit_eof"])
                    self.assertTrue(result["control_eof"])
                    self.assertEqual(bool(result["io_errors"]), failure != "inherited-holder")
                    for fd in created:
                        with self.assertRaises(OSError) as error: os.fstat(fd)
                        self.assertEqual(error.exception.errno, errno.EBADF)
                finally:
                    left.close(); right.close()
                    for child in children:
                        if child.returncode is None:
                            # The test still owns this unreaped direct child on a red run.
                            try: real_killpg(child.pid, signal.SIGKILL)
                            except ProcessLookupError: pass
                            child.wait()
                    for fd in created:
                        try: os.close(fd)
                        except OSError: pass

    def test_controller_loss_keeps_cleanup_owner_and_stopped_owner_can_resume(self):
        self.assertTrue(self.run_case("hang", disconnect=True)["controller_lost"])
        self.assertEqual(self.run_case("tty", stop_owner=True)["waitpid"], 0)


class BoundedCommandTests(unittest.TestCase):
    def command_case(self, case):
        children, stdout_fds, selectors_created, signals, output = [], [], [], [], bytearray()
        real_popen, real_selector = adapter.subprocess.Popen, adapter.selectors.DefaultSelector
        real_killpg, real_read = adapter.os.killpg, adapter.os.read
        exited_before_read, wait_calls = False, 0

        def popen(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child); stdout_fds.append(child.stdout.fileno())
            if case.startswith("wait-"):
                real_wait = child.wait
                def interrupted_wait(*args, **kwargs):
                    nonlocal wait_calls
                    wait_calls += 1
                    if wait_calls == 1:
                        self.assertIsNone(child.returncode)
                        raise InterruptedError("fixed final wait interruption")
                    return real_wait(*args, **kwargs)
                child.wait = interrupted_wait
            return child

        def killpg(pid, sig):
            child = children[0]
            self.assertIsNone(child.returncode, "group signal after Popen reaped the leader")
            try:
                os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            except ChildProcessError:
                self.fail("group signal after actual waitpid")
            signals.append(sig)
            return real_killpg(pid, sig)

        def read(fd, count):
            data = real_read(fd, count)
            if children and fd == children[0].stdout.fileno(): output.extend(data)
            return data

        class Selector:
            def __init__(self):
                if case == "construct": raise OSError(errno.EIO, "fixed selector construct failure")
                self.inner, self.closed = real_selector(), False
                selectors_created.append(self)
            def register(self, *args):
                if case in ("register", "wait-after-register-fault"): raise OSError(errno.EIO, "fixed selector register failure")
                return self.inner.register(*args)
            def unregister(self, *args): return self.inner.unregister(*args)
            def select(self, *args):
                nonlocal exited_before_read
                if case == "close-after-read-fault": raise OSError(errno.EIO, "fixed command read failure")
                if case in ("exited-cap", "holder") and not exited_before_read:
                    deadline = time.monotonic() + 5
                    while os.waitid(os.P_PID, children[0].pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None:
                        if time.monotonic() >= deadline: raise AssertionError("fixed leader did not exit")
                        select.select([], [], [], 0.01)
                    exited_before_read = True
                return self.inner.select(*args)
            def close(self):
                self.inner.close(); self.closed = True
                if case.startswith("close-"): raise OSError(errno.EIO, "fixed selector close failure")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            listener, holder = None, None
            code, args, timeout, limit = "import signal; signal.pause()", [], 1, 32
            expected = f"fixed selector {case} failure"
            if case == "close-after-success":
                code, expected = "print('fixed')", "fixed selector close failure"
            elif case == "wait-after-success":
                code, expected = "print('fixed')", "fixed final wait interruption"
            elif case == "wait-after-register-fault":
                expected = "fixed selector register failure"
            elif case == "close-after-read-fault":
                expected = "fixed command read failure"
            elif case == "exited-cap":
                code, expected = "import os; os.write(1,b'x'*64)", "physical cap"
            elif case == "holder":
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(root / "holder.sock")); listener.listen(1); listener.settimeout(5)
                code = """
import os, socket, sys
ready_r, ready_w = os.pipe()
if os.fork():
    os.close(ready_w)
    assert os.read(ready_r, 1) == b'1'
    os.write(1, b'holder-ready')
    os._exit(0)
os.close(ready_r)
with socket.socket(socket.AF_UNIX) as channel:
    channel.connect(sys.argv[1])
    channel.sendall(b'ready')
    os.write(ready_w, b'1')
    channel.recv(1)
os._exit(0)
"""
                args, expected = [str(root / "holder.sock")], "deadline"
            try:
                with patch.object(adapter.subprocess, "Popen", side_effect=popen), patch.object(adapter.selectors, "DefaultSelector", Selector), \
                     patch.object(adapter.os, "killpg", side_effect=killpg), patch.object(adapter.os, "read", side_effect=read):
                    with self.assertRaisesRegex((OSError, ValueError), expected):
                        adapter.bounded_command([sys.executable, "-c", code, *args], {"PATH": "/usr/bin:/bin"}, root, timeout=timeout, limit=limit)
                self.assertEqual(len(children), 1)
                child = children[0]
                self.assertIsNotNone(child.returncode)
                with self.assertRaises(ChildProcessError): os.waitpid(child.pid, os.WNOHANG)
                self.assertTrue(child.stdout.closed)
                with self.assertRaises(OSError) as error: os.fstat(stdout_fds[0])
                self.assertEqual(error.exception.errno, errno.EBADF)
                self.assertTrue(all(s.closed for s in selectors_created))
                self.assertTrue(all(s.inner.get_map() is None for s in selectors_created))
                if case.startswith("wait-"):
                    self.assertGreaterEqual(wait_calls, 2)
                    if case == "wait-after-success": self.assertEqual(signals, [])
                if case in ("exited-cap", "holder"):
                    self.assertTrue(exited_before_read)
                    self.assertEqual(child.returncode, 0)
                    self.assertIn(signal.SIGKILL, signals)
                    self.assertEqual(bytes(output), b"x" * 33 if case == "exited-cap" else b"holder-ready")
                if listener:
                    holder, _ = listener.accept(); holder.settimeout(5)
                    self.assertEqual(holder.recv(5), b"ready")
                    self.assertEqual(holder.recv(1), b"", "inherited stdout holder survived owned group cleanup")
            finally:
                # Red runs retain real child handles; never signal a released numeric group.
                for child in children:
                    if child.returncode is None:
                        try: real_killpg(child.pid, signal.SIGKILL)
                        except (ProcessLookupError, PermissionError): pass
                        finally: child.wait()
                    child.stdout.close()
                for selected in selectors_created:
                    if not selected.closed: selected.inner.close()
                if listener:
                    if holder is None:
                        holder, _ = listener.accept(); holder.settimeout(5)
                        self.assertEqual(holder.recv(5), b"ready")
                    try: holder.sendall(b"release")
                    except BrokenPipeError: pass
                    self.assertEqual(holder.recv(1), b"")
                    holder.close(); listener.close()

    def test_setup_and_selector_close_failures_reap_and_close_stdout(self):
        for case in ["construct", "register", "close-after-success", "close-after-read-fault"]:
            with self.subTest(case=case): self.command_case(case)

    def test_final_wait_interruption_retains_owner_and_primary_failure(self):
        for case in ["wait-after-success", "wait-after-register-fault"]:
            with self.subTest(case=case): self.command_case(case)

    def test_exited_leader_output_cap_never_signals_after_reap(self):
        self.command_case("exited-cap")

    def test_inherited_stdout_holder_is_terminated_before_leader_reap(self):
        self.command_case("holder")


class PureTests(unittest.TestCase):
    def test_private_artifacts_and_exclusive_admission(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            path = root / "receipt.json"
            adapter.publish(path, {"attempt": 1}, 0o400)
            self.assertEqual(adapter.read_json(path), {"attempt": 1})
            with self.assertRaises(FileExistsError):
                adapter.publish(path, {"attempt": 2})
            link = root / "link"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                adapter.read_json(link)
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                adapter.read_json(path)
            path.chmod(0o600)
            path.write_bytes(b"x" * 100)
            with self.assertRaises(ValueError):
                adapter.read_bytes(path, 99)

    def test_exact_approval_attempt_admission_and_failed_baseline_stop(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            manifest = {"kind": adapter.KIND, "version": 1, "roles": ["baseline", "observed"], "root": str(root), "config": {}, "tools": {}, "sources": {}}
            adapter.publish(root / "manifest.json", manifest, 0o400)
            (root / "pi").write_text("fixed test shim")
            (root / "pi").chmod(0o500)
            (root / "run").write_text("fixed test runner")
            (root / "run").chmod(0o500)
            adapter.publish(root / "seal.json", {"manifest": adapter.digest(manifest), "launcher": adapter.file_hash(root / "pi"), "runner": adapter.file_hash(root / "run")})
            approval = {"kind": adapter.KIND, "manifest": adapter.digest(manifest), "role": "baseline", "ordinal": 1,
                        "approve_native_execution": True, "approve_synthetic_pricing": True}
            with patch.object(adapter, "verify_tools", return_value={}), patch.object(adapter, "source_seal", return_value={}):
                for key in approval:
                    changed = {**approval, key: None}
                    path = root / f"bad-{key}.json"
                    adapter.publish(path, changed)
                    with self.assertRaises(ValueError):
                        adapter.admit(root / "manifest.json", path, "baseline", 1)
                    self.assertFalse((root / "admitted-1.json").exists())
                adapter.publish(root / "approval.json", approval)
                (root / "pi").chmod(0o555)
                with self.assertRaisesRegex(ValueError, "private immutable launcher"):
                    adapter.admit(root / "manifest.json", root / "approval.json", "baseline", 1)
                (root / "pi").chmod(0o500)
                with patch.object(adapter, "source_seal", return_value={"changed": True}):
                    with self.assertRaisesRegex(ValueError, "changed pinned input"):
                        adapter.admit(root / "manifest.json", root / "approval.json", "baseline", 1)
                adapter.admit(root / "manifest.json", root / "approval.json", "baseline", 1)
                self.assertEqual(adapter.read_json(root / "admitted-1.json"), approval)
                with self.assertRaises(FileExistsError):
                    adapter.admit(root / "manifest.json", root / "approval.json", "baseline", 1)
                (root / "baseline").mkdir(mode=0o700)
                adapter.publish(root / "baseline/report.json", {"classification": "native_inconclusive", "manifest": adapter.digest(manifest)})
                adapter.publish(root / "observed-approval.json", {**approval, "role": "observed", "ordinal": 2})
                with self.assertRaises(ValueError):
                    adapter.admit(root / "manifest.json", root / "observed-approval.json", "observed", 2)
                self.assertFalse((root / "admitted-2.json").exists())

    def test_symlink_dependency_contents_and_cycles_are_sealed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            deps, package = root / "node_modules", root / "package"
            deps.mkdir(); package.mkdir()
            (package / "index.js").write_text("original")
            (deps / "linked").symlink_to(package, target_is_directory=True)
            (package / "cycle").symlink_to(deps, target_is_directory=True)
            before = adapter.dependency_hash(deps)
            self.assertEqual(adapter.dependency_hash(deps), before)
            (deps / "linked/index.js").write_text("changed through directory symlink")
            self.assertNotEqual(adapter.dependency_hash(deps), before)

    def test_streaming_output_cap_and_hanging_command_reaping(self):
        env = {"PATH": "/usr/bin:/bin"}
        with tempfile.TemporaryDirectory() as directory:
            for code, pattern in [("import os; os.write(1,b'x'*1000000)", "physical cap"),
                                  ("import signal; signal.pause()", "deadline")]:
                with self.assertRaisesRegex(ValueError, pattern):
                    adapter.bounded_command([sys.executable, "-c", code], env, directory, timeout=0.2, limit=1024)
            self.assertEqual(adapter.bounded_command([sys.executable, "-c", "print('fixed')"], env, directory), b"fixed\n")

    def test_fixed_helpers_ignore_stale_bytecode_and_ambient_pythonpath(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for name in [PATH.name, "aoe-afk-native-controller.py"]:
                source = root / name
                source.write_bytes(PATH.with_name(name).read_bytes())
                cache = Path(importlib.util.cache_from_source(str(source)))
                cache.parent.mkdir(exist_ok=True)
                meta = source.stat()
                poison = compile("raise RuntimeError('unsealed cache executed')", str(source), "exec")
                cache.write_bytes(importlib.util.MAGIC_NUMBER + struct.pack("<III", 0, int(meta.st_mtime), meta.st_size) + marshal.dumps(poison))
            ambient = root / "ambient"
            ambient.mkdir()
            marker = root / "ambient-executed"
            (ambient / "sitecustomize.py").write_text(f"open({str(marker)!r}, 'w').write('site')")
            (ambient / "json.py").write_text("raise RuntimeError('ambient json executed')")
            before = {str(p): p.read_bytes() for p in root.rglob("*.pyc")}
            code = "import runpy,sys; c=runpy.run_path(sys.argv[1]); m=runpy.run_path(sys.argv[2]); loaded=m['load_controller'](); assert loaded.a.KIND==c['a'].KIND; assert sys.flags.isolated and sys.flags.no_site and sys.flags.dont_write_bytecode; print(c['a'].KIND)"
            result = subprocess.run([sys.executable, "-I", "-S", "-B", "-c", code, str(root / "aoe-afk-native-controller.py"), str(root / PATH.name)],
                                    env={**os.environ, "PYTHONPATH": str(ambient), "PYTHONSTARTUP": str(ambient / "sitecustomize.py")},
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(result.stdout.strip(), adapter.KIND.encode())
            self.assertFalse(marker.exists())
            self.assertEqual(before, {str(p): p.read_bytes() for p in root.rglob("*.pyc")})

    def test_no_inherited_credentials_and_explicit_proposal(self):
        manifest = {"sources": {"pi": {"path": "/source"}}}
        env = adapter.isolated_env("/private/attempt", manifest)
        self.assertNotIn("NODE_OPTIONS", env)
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertEqual(env["PI_OFFLINE"], "1")
        proposed = adapter.proposal()
        self.assertEqual(proposed["usage_guard"]["usd"]["limit_nano"], 100)
        self.assertEqual(proposed["grant"]["question_deferrals"], 1)
        self.assertEqual(proposed["grant"]["settlement_nudges"], 0)
        self.assertEqual(proposed["grant"]["requests"], 2)


if __name__ == "__main__":
    unittest.main()
