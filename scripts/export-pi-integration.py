#!/usr/bin/env python3
"""Export a build inventory, not permission to load Pi extensions."""
import argparse
import hashlib
import json
from pathlib import Path
import re


def regular_bytes(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"expected a regular source file: {path}")
    return path.read_bytes()


def export(source, binary, output, system, revision):
    source, binary, output = Path(source), Path(binary), Path(output)
    assets = source / "assets/session"
    if assets.is_symlink():
        raise ValueError("integration asset directory must not be a symlink")
    emitter = regular_bytes(source / "src/session/afk.rs").decode()
    expressions = re.findall(r'include_bytes!\s*\(\s*"([^"]+)"\s*\)', emitter)
    if len(expressions) != len(re.findall(r"\binclude_bytes!", emitter)) or not expressions:
        raise ValueError("AFK asset emitter changed: inventory requires literal include_bytes paths")
    afk = {}
    for relative in expressions:
        path = Path(relative)
        if path.parent.as_posix() != "../../assets/session":
            raise ValueError(f"unexpected embedded asset location: {relative}")
        if path.name in afk:
            raise ValueError(f"duplicate embedded asset: {relative}")
        afk[path.name] = regular_bytes(assets / path.name)
    if "aoe-afk-runtime.mjs" not in afk:
        raise ValueError("AFK entrypoint is not embedded by the selected source")
    identity = {"pi-aoe-session-id.js": regular_bytes(assets / "aoe-session-id.js")}
    bundles = {"identity": identity, "afk": afk}
    manifest = {
        "version": 1,
        "source_revision": revision,
        "system": system,
        "binary": {"sha256": hashlib.sha256(regular_bytes(binary)).hexdigest()},
        "bundles": {
            name: {
                "entrypoint": "pi-aoe-session-id.js" if name == "identity" else "aoe-afk-runtime.mjs",
                "files": {filename: hashlib.sha256(data).hexdigest() for filename, data in sorted(files.items())},
            }
            for name, files in bundles.items()
        },
    }
    output.mkdir(parents=True, exist_ok=False)
    for files in bundles.values():
        for filename, data in files.items():
            (output / filename).write_bytes(data)
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--system", required=True)
    parser.add_argument("--revision", default=None)
    args = parser.parse_args()
    export(args.source, args.binary, args.output, args.system, args.revision)


if __name__ == "__main__":
    main()
