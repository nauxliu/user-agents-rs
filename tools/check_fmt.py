#!/usr/bin/env python3
"""Check Rust formatting, using the caller's selected Rust toolchain."""
from pathlib import Path
import subprocess

raise SystemExit(subprocess.call(["cargo", "fmt", "--all", "--check"],
                                cwd=Path(__file__).resolve().parents[1]))
