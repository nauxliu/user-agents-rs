#!/usr/bin/env python3
"""Sync Intoli's gzip dataset only when its reviewed source manifest still matches.

Never imports, installs, builds, or executes anything from the upstream checkout.
Python 3.10+ and Git are the only requirements.
"""

import argparse
import gzip
import hashlib
import io
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DATA_PATH = "src/user-agents.json.gz"
REPOSITORY = "https://github.com/intoli/user-agents.git"
BRANCH = "main"
# Every other tracked file is monitored, including new files, dependency locks,
# tests, CI configuration, file modes, and LICENSE. No broad source-path allowlist.
IGNORED = {
    DATA_PATH,
    "README.md",
    "CONTRIBUTING.md",
    "CLA.md",
    ".github/pull_request_template.md",
    "media/facebook.png",
    "media/reddit.png",
    "media/twitter.png",
    "media/ycombinator.png",
}


class CodeChanged(RuntimeError):
    pass


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args])


def sha256(content):
    return hashlib.sha256(content).hexdigest()


def source_manifest(repo):
    result = {}
    entries = git(repo, "ls-tree", "-rz", "HEAD").split(b"\0")
    for entry in entries:
        if not entry:
            continue
        info, path_bytes = entry.split(b"\t", 1)
        mode, kind, oid = info.decode().split()
        path = path_bytes.decode("utf-8")
        if path in IGNORED:
            continue
        if kind != "blob" or mode not in {"100644", "100755"}:
            raise CodeChanged(f"Unsupported upstream entry: {path} ({mode}, {kind})")
        content = git(repo, "cat-file", "blob", oid)
        if path == "package.json":
            package = json.loads(content)
            # Upstream bumps this for every daily data release.
            package.pop("version", None)
            content = json.dumps(package, sort_keys=True, separators=(",", ":")).encode()
        result[path] = {"mode": mode, "sha256": sha256(content)}
    return dict(sorted(result.items()))


def dump_json(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def review(repo, root):
    """Explicit maintainer operation, never called by the scheduled workflow."""
    lock = {
        "repository": REPOSITORY,
        "branch": BRANCH,
        "reviewed_commit": git(repo, "rev-parse", "HEAD").decode().strip(),
        "files": source_manifest(repo),
    }
    (root / "upstream.lock.json").write_bytes(dump_json(lock))
    print(f"Recorded reviewed source at {lock['reviewed_commit']}")


def validate_data(content):
    if len(content) > 16 * 1024 * 1024:
        raise ValueError("Compressed dataset exceeds 16 MiB")
    with gzip.GzipFile(fileobj=io.BytesIO(content)) as stream:
        raw = stream.read(64 * 1024 * 1024 + 1)
    if len(raw) > 64 * 1024 * 1024:
        raise ValueError("Decompressed dataset exceeds 64 MiB")
    def finite_float(value):
        number = float(value)
        if not math.isfinite(number):
            raise ValueError(f"Nonfinite number in dataset: {value}")
        return number

    def reject_constant(value):
        raise ValueError(f"Invalid JSON constant: {value}")

    profiles = json.loads(raw, parse_float=finite_float, parse_constant=reject_constant)
    if not isinstance(profiles, list) or not profiles:
        raise ValueError("Dataset must be a nonempty array")
    positive = False
    for index, profile in enumerate(profiles):
        if not isinstance(profile, dict):
            raise ValueError(f"Profile {index} must be an object")
        for field in ("appName", "platform", "userAgent", "vendor", "deviceCategory"):
            if not isinstance(profile.get(field), str):
                raise ValueError(f"Invalid {field} at profile {index}")
        if not profile["userAgent"]:
            raise ValueError(f"Empty userAgent at profile {index}")
        for field in ("pluginsLength", "screenHeight", "screenWidth", "viewportHeight", "viewportWidth"):
            value = profile.get(field)
            if type(value) is not int or not 0 <= value <= 2**64 - 1:
                raise ValueError(f"Invalid {field} at profile {index}")
        weight = profile.get("weight")
        if type(weight) not in (int, float) or not math.isfinite(weight) or weight < 0:
            raise ValueError(f"Invalid weight at profile {index}")
        positive |= weight > 0
    if not positive:
        raise ValueError("Dataset has no positive weight")
    return len(profiles)


def sync(repo, root):
    lock = json.loads((root / "upstream.lock.json").read_bytes())
    manifest = source_manifest(repo)
    changed = sorted(path for path in lock["files"].keys() | manifest.keys()
                     if lock["files"].get(path) != manifest.get(path))
    if changed:
        revision = git(repo, "rev-parse", "HEAD").decode().strip()
        raise CodeChanged(
            "Upstream code changed; data sync blocked. Review and port changes before "
            "updating upstream.lock.json.\n"
            f"Reviewed: {lock['reviewed_commit']}\nCurrent: {revision}\n"
            "Changed paths:\n" + "\n".join(f"  {path}" for path in changed)
        )

    entry = git(repo, "ls-tree", "HEAD", "--", DATA_PATH).decode().split()
    if len(entry) != 4 or entry[0] != "100644" or entry[1] != "blob":
        raise ValueError("Upstream dataset is missing or is not a regular file")
    content = git(repo, "cat-file", "blob", entry[2])
    count = validate_data(content)
    target = root / "data/user-agents.json.gz"
    if target.exists() and target.read_bytes() == content:
        print(f"Dataset unchanged ({count} profiles); upstream code matches reviewed baseline.")
        return False
    metadata = {
        "repository": lock["repository"],
        "commit": git(repo, "rev-parse", "HEAD").decode().strip(),
        "sha256": sha256(content),
        "profiles": count,
    }
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(content)
    (root / "data/upstream.json").write_bytes(dump_json(metadata))
    print(f"Updated {count} profiles from {metadata['commit']}.")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["sync", "review"])
    parser.add_argument("--checkout", type=Path, help="Use HEAD of an existing upstream checkout")
    args = parser.parse_args()
    if args.command == "review":
        if args.checkout is None:
            parser.error("review requires --checkout pointing to manually reviewed upstream source")
        review(args.checkout, ROOT)
    elif args.checkout is not None:
        sync(args.checkout, ROOT)
    else:
        lock = json.loads((ROOT / "upstream.lock.json").read_bytes())
        # Only clone the known public repository. Never execute its code.
        if lock["repository"] != REPOSITORY or lock["branch"] != BRANCH:
            raise ValueError("Unexpected upstream repository or branch")
        with tempfile.TemporaryDirectory(prefix="user-agents-upstream-") as directory:
            repo = Path(directory) / "upstream"
            subprocess.run(["git", "clone", "--depth", "1", "--single-branch", "--no-checkout",
                            "--branch", BRANCH, REPOSITORY, str(repo)], check=True, timeout=180)
            sync(repo, ROOT)


if __name__ == "__main__":
    try:
        main()
    except (CodeChanged, ValueError, OSError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print("::error::Upstream sync failed; inspect the log and UPSTREAM.md.", file=sys.stderr)
        sys.exit(1)
