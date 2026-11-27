#!/usr/bin/env python3
"""Reserve swap for the release build on ephemeral GitHub-hosted Linux runners."""

import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

GIB = 1024**3


def capacity(root):
    fields = dict(line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())
    return (int(fields["MemTotal"].split()[0]) * 1024,
            int(fields["SwapTotal"].split()[0]) * 1024, shutil.disk_usage(root).free)


def reservation(ram, swap, free):
    needed = max(0, (24 * GIB - ram - swap + GIB - 1) // GIB * GIB)
    if needed > 20 * GIB or free - needed < 8 * GIB:
        raise RuntimeError("Nix needs 24 GiB RAM+swap and 8 GiB remaining disk; refusing an unsafe build")
    return needed


def save(state, data, *, initial=False):
    flags = os.O_WRONLY | os.O_NOFOLLOW | (os.O_CREAT | os.O_EXCL if initial else os.O_TRUNC)
    with os.fdopen(os.open(state, flags, 0o600), "w") as stream:
        json.dump(data, stream, indent=2)
        stream.write("\n")


def identity(path):
    leaf, parent = path.lstat(), path.parent.lstat()
    return {"device": leaf.st_dev, "inode": leaf.st_ino,
            "parent_device": parent.st_dev, "parent_inode": parent.st_ino}


def privileged(operation, path, *arguments):
    subprocess.run(["sudo", "-n", "--", operation, "--", *arguments, str(path)], check=True)


def prepare(state, root):
    if sys.platform != "linux" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("Swap reservation is only allowed on an ephemeral GitHub-hosted Linux runner")
    state, root = Path(state), Path(root).resolve(strict=True)
    ram, swap, free = capacity(root)
    state.parent.mkdir(parents=True, exist_ok=True)
    data = {"ram_bytes": ram, "existing_swap_bytes": swap, "free_disk_bytes": free,
            "swap": None, "cleaned": False}
    save(state, data, initial=True)
    needed = reservation(ram, swap, free)
    if not needed:
        print("Nix memory: existing RAM+swap already meets 24 GiB", flush=True)
        return
    path = Path(tempfile.mkdtemp(prefix="aoe-nix-memory-", dir=root)) / "swap"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.posix_fallocate(fd, 0, needed)
    finally:
        os.close(fd)
        data.update(identity(path))
        data.update({"swap": str(path), "size": path.stat().st_size, "requested_bytes": needed})
        save(state, data)
    privileged("chown", path, "0:0")
    privileged("mkswap", path)
    privileged("swapon", path)
    print(f"Nix memory: added {needed // GIB} GiB owned swap; kept existing swap unchanged", flush=True)


def active_swaps():
    return {re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), line.split()[0])
            for line in Path("/proc/swaps").read_text().splitlines()[1:] if line.split()}


def cleanup(state, root):
    state, root = Path(state), Path(root).resolve(strict=True)
    if not state.exists() and not state.is_symlink():
        return
    record = state.lstat()
    if (not stat.S_ISREG(record.st_mode) or record.st_uid != os.getuid()
            or record.st_nlink != 1 or stat.S_IMODE(record.st_mode) != 0o600):
        raise RuntimeError("Unexpected Nix memory receipt ownership")
    data = json.loads(state.read_text())
    if data["cleaned"]:
        return
    if data["swap"] is not None:
        path = Path(data["swap"])
        if (path.name != "swap" or path.parent.parent != root
                or not path.parent.name.startswith("aoe-nix-memory-")):
            raise RuntimeError("Nix memory reservation is outside its owned directory")
        leaf, parent = path.lstat(), path.parent.lstat()
        if (identity(path) != {key: data[key] for key in identity(path)}
                or not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid()
                or stat.S_IMODE(parent.st_mode) != 0o700
                or not stat.S_ISREG(leaf.st_mode) or leaf.st_uid not in (0, os.getuid())
                or leaf.st_nlink != 1 or stat.S_IMODE(leaf.st_mode) != 0o600
                or leaf.st_size != data["size"]):
            raise RuntimeError("Nix memory reservation identity changed; retaining it")
        if str(path) in active_swaps():
            privileged("swapoff", path)
        path.unlink()
        path.parent.rmdir()
    data["cleaned"] = True
    save(state, data)
    print("Nix memory: owned reservation released", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("prepare", "cleanup"):
        sys.exit("usage: nix-ci-memory.py prepare|cleanup RECEIPT")
    os.umask(0o077)
    operation = prepare if sys.argv[1] == "prepare" else cleanup
    operation(Path(sys.argv[2]), Path(os.environ["RUNNER_TEMP"]))
