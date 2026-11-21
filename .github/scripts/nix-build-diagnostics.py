#!/usr/bin/env python3
"""Sample hosted-runner resources while a foreground command runs unchanged."""

import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import threading
import time


def read_fields(path):
    result = {}
    try:
        for line in path.read_text().splitlines():
            key, _, value = line.partition(":")
            if value.strip():
                result[key] = value.split()[0]
    except OSError:
        pass
    return result


def resources(proc=Path("/proc"), cgroup=Path("/sys/fs/cgroup")):
    memory = read_fields(proc / "meminfo")
    snapshot = {
        "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "cpu_count": os.cpu_count(),
        "load": os.getloadavg(),
        "memory_kib": {key: int(memory[key]) for key in (
            "MemTotal", "MemAvailable", "SwapTotal", "SwapFree"
        ) if key in memory},
    }
    try:
        ticks = [int(n) for n in (proc / "stat").read_text().splitlines()[0].split()[1:9]]
        snapshot["cpu_ticks"] = {"total": sum(ticks), "idle": sum(ticks[3:5])}
    except (OSError, ValueError, IndexError):
        pass
    processes = []
    for entry in proc.glob("[0-9]*"):
        fields = read_fields(entry / "status")
        if "VmRSS" in fields:
            processes.append({"pid": int(entry.name), "name": fields.get("Name", "?"),
                              "rss_kib": int(fields["VmRSS"])})
    snapshot["largest_processes"] = sorted(processes, key=lambda p: -p["rss_kib"])[:8]
    snapshot["disk_bytes"] = {}
    for path in ("/", "/nix"):
        try:
            usage = shutil.disk_usage(path)
            snapshot["disk_bytes"][path] = {"total": usage.total, "free": usage.free}
        except OSError:
            pass
    try:
        relative = next(line.split(":", 2)[2] for line in
                        (proc / "self/cgroup").read_text().splitlines()
                        if line.startswith("0::"))
        group = cgroup / relative.lstrip("/")
        snapshot["cgroup_memory"] = {}
        for name in ("memory.current", "memory.max", "memory.events"):
            try:
                snapshot["cgroup_memory"][name] = (group / name).read_text().strip()
            except OSError:
                pass
    except (OSError, StopIteration):
        pass
    return snapshot


def kernel_oom():
    for command in (["dmesg", "--color=never"], ["sudo", "-n", "dmesg", "--color=never"]):
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=3)
        except (OSError, subprocess.TimeoutExpired):
            continue
        if result.returncode == 0:
            return {"matches": [line[:500] for line in result.stdout.splitlines()
                                if re.search(r"out of memory|oom-kill|killed process|memory cgroup out", line, re.I)][-20:]}
    return {"unavailable": "kernel log access denied or unavailable"}


def emit(value):
    print("[nix-resource] " + json.dumps(value, sort_keys=True), flush=True)


def monitor(stop):
    previous = None
    while not stop.is_set():
        try:
            sample = resources()
            ticks = sample.get("cpu_ticks")
            if previous and ticks:
                total = ticks["total"] - previous["total"]
                if total > 0:
                    sample["cpu_busy_percent"] = round(
                        100 * (1 - (ticks["idle"] - previous["idle"]) / total), 1)
            previous = ticks
            sample["kernel_oom"] = kernel_oom()
            emit(sample)
        except Exception as error:
            emit({"sampler_error": str(error)})
        stop.wait(30)


def run(command):
    stop = threading.Event()
    sampler = threading.Thread(target=monitor, args=(stop,), daemon=True)
    child = None
    pending_signal = None

    def forward(number, _frame):
        nonlocal pending_signal
        if child is None:
            pending_signal = number
            return
        try:
            child.send_signal(number)
        except ProcessLookupError:
            pass

    handlers = {number: signal.signal(number, forward)
                for number in (signal.SIGINT, signal.SIGTERM)}
    sampler.start()
    try:
        if pending_signal is not None:
            return 128 + pending_signal
        # No new session or detached process: wait for the original command.
        with subprocess.Popen(command) as process:
            child = process
            if pending_signal is not None:
                forward(pending_signal, None)
            status = child.wait()
        return status if status >= 0 else 128 - status
    finally:
        stop.set()
        sampler.join(timeout=8)
        try:
            emit({"final": resources(), "kernel_oom": kernel_oom()})
        except Exception as error:
            emit({"sampler_error": str(error)})
        for number, handler in handlers.items():
            signal.signal(number, handler)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: nix-build-diagnostics.py COMMAND [ARG ...]")
    sys.exit(run(sys.argv[1:]))
