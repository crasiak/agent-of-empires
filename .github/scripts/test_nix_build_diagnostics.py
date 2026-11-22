#!/usr/bin/env python3
"""Behavioral checks for the foreground build diagnostics wrapper."""

from collections import deque
from contextlib import redirect_stdout
import importlib.util
import io
from pathlib import Path
import queue
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("nix-build-diagnostics.py")
SPEC = importlib.util.spec_from_file_location("nix_diagnostics", SCRIPT)
diag = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diag)


class DiagnosticsTests(unittest.TestCase):
    def test_reports_pressure_without_process_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            proc = root / "proc"
            (proc / "self").mkdir(parents=True)
            (proc / "meminfo").write_text("MemTotal: 1000 kB\nMemAvailable: 25 kB\nSwapFree: 0 kB\n")
            (proc / "stat").write_text("cpu 10 20 30 40 50 60 70 80 90 100\n")
            (proc / "self/cgroup").write_text("0::/job\n")
            for pid, rss in ((12, 200), (13, 500)):
                (proc / str(pid)).mkdir()
                (proc / str(pid) / "status").write_text(f"Name:\trustc\nVmRSS:\t{rss} kB\n")
                (proc / str(pid) / "cmdline").write_text("private-command-token")
            group = root / "cgroup/job"
            group.mkdir(parents=True)
            (group / "memory.events").write_text("oom 1\noom_kill 1\n")
            result = diag.resources(proc, root / "cgroup")
            self.assertEqual(result["memory_kib"]["MemAvailable"], 25)
            self.assertEqual(result["cpu_ticks"], {"total": 360, "idle": 90})
            self.assertEqual([p["pid"] for p in result["largest_processes"]], [13, 12])
            self.assertIn("oom_kill 1", result["cgroup_memory"]["memory.events"])
            self.assertNotIn("private-command-token", str(result))

    def test_diagnostic_failure_does_not_replace_command_status(self):
        for status in (0, 17):
            with self.subTest(status=status), redirect_stdout(io.StringIO()) as output:
                with patch.object(diag, "monitor", lambda stop: None), \
                     patch.object(diag, "resources", side_effect=OSError("metrics unavailable")):
                    actual = diag.run([sys.executable, "-c", f"raise SystemExit({status})"])
                self.assertEqual(actual, status)
                self.assertIn("metrics unavailable", output.getvalue())

    def test_kernel_evidence_is_filtered_bounded_and_noninteractive(self):
        output = "unrelated-private-sentinel\n" + "\n".join(
            f"oom-kill {index}: " + "x" * 600 for index in range(25)
        )
        success = subprocess.CompletedProcess([], 0, output, "")
        failures = (
            subprocess.CompletedProcess([], 1, "", "denied"),
            subprocess.TimeoutExpired("dmesg", 3),
            FileNotFoundError("dmesg"),
        )
        with patch.object(diag.subprocess, "run", return_value=success) as run:
            result = diag.kernel_oom()
            self.assertEqual(run.call_count, 1)
        self.assertEqual(len(result["matches"]), 20)
        self.assertTrue(result["matches"][0].startswith("oom-kill 5:"))
        self.assertTrue(all(len(line) == 500 for line in result["matches"]))
        self.assertNotIn("unrelated-private-sentinel", str(result))
        for failure in failures:
            with self.subTest(failure=type(failure).__name__):
                with patch.object(diag.subprocess, "run", side_effect=[failure, success]) as run:
                    self.assertEqual(diag.kernel_oom(), result)
                self.assertEqual(run.call_args_list[1].args[0][:2], ["sudo", "-n"])
        with patch.object(diag.subprocess, "run", side_effect=OSError("unavailable")):
            self.assertIn("unavailable", diag.kernel_oom())

    def test_signal_after_child_assignment_is_not_forwarded_twice(self):
        class Child:
            def __init__(self):
                self.signals = []

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def send_signal(self, number):
                self.signals.append(number)

            def wait(self):
                return 23

        child = Child()
        delivered = False

        def inject_signal(frame, event, _argument):
            nonlocal delivered
            if (event == "line" and frame.f_code is diag.run.__code__
                    and frame.f_locals.get("child") is child and not delivered):
                delivered = True
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return inject_signal

        previous = sys.gettrace()
        try:
            with patch.object(diag, "monitor", lambda stop: None), \
                 patch.object(diag, "resources", return_value={}), \
                 patch.object(diag, "kernel_oom", return_value={}), \
                 patch.object(diag.subprocess, "Popen", return_value=child), \
                 redirect_stdout(io.StringIO()):
                sys.settrace(inject_signal)
                status = diag.run(["unused"])
        finally:
            sys.settrace(previous)
        self.assertTrue(delivered)
        self.assertEqual(status, 23)
        self.assertEqual(child.signals, [signal.SIGTERM])

    def test_termination_reaches_child_and_waits_for_its_status(self):
        for number in (signal.SIGINT, signal.SIGTERM):
            for handled in (False, True):
                with self.subTest(signal=number, handled=handled):
                    self.check_termination(number, handled)

    def check_termination(self, number, handled):
        handler = "lambda *_: sys.exit(23)" if handled else "signal.SIG_DFL"
        child = (
            "import os,signal,sys; "
            f"signal.signal({number}, {handler}); "
            "print('CHILD_READY:'+str(os.getpid()), flush=True); signal.pause()"
        )
        wrapper = (
            "import faulthandler,runpy,sys\n"
            "script = sys.argv.pop(1)\n"
            "faulthandler.dump_traceback_later(8, repeat=True)\n"
            "try:\n"
            "    runpy.run_path(script, run_name='__main__')\n"
            "finally:\n"
            "    faulthandler.cancel_dump_traceback_later()\n"
        )
        process = subprocess.Popen(
            [sys.executable, "-c", wrapper, str(SCRIPT), sys.executable, "-c", child],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        lines = queue.Queue()
        output = deque(maxlen=100)
        output_lock = threading.Lock()

        def captured_output():
            with output_lock:
                return "".join(output)

        def read_output():
            for line in process.stdout:
                with output_lock:
                    output.append(line)
                lines.put(line)

        reader = threading.Thread(target=read_output, daemon=True)
        reader.start()
        child_pid = None
        try:
            deadline = time.monotonic() + 15
            while child_pid is None:
                remaining = deadline - time.monotonic()
                self.assertGreater(remaining, 0, "child did not become ready")
                line = lines.get(timeout=remaining)
                if line.startswith("CHILD_READY:"):
                    child_pid = int(line.partition(":")[2])
            process.send_signal(number)
            try:
                status = process.wait(timeout=20)
            except subprocess.TimeoutExpired as error:
                error.add_note("Wrapper output before fixture cleanup:\n" + captured_output())
                raise
            self.assertEqual(status, 23 if handled else 128 + number, captured_output())
        finally:
            if process.poll() is None:
                if child_pid is not None:
                    try:
                        import os
                        os.kill(child_pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                process.kill()
                process.wait()
            reader.join(timeout=5)
            process.stdout.close()


if __name__ == "__main__":
    unittest.main()
