#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent


def run(label: str, command: list[str]) -> None:
    print(f"[check] {label}: {' '.join(command)}", flush=True)
    raise_on = subprocess.run(command, cwd=ROOT, check=False).returncode
    if raise_on:
        raise SystemExit(raise_on)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "mode",
        nargs="?",
        choices=("check", "verify", "deep", "fix", "canon"),
        default="check",
    )
    mode = parser.parse_args().mode
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    meta = manifest["package"]["metadata"]["rust-starter"]

    if mode != "verify":
        for slot, command in enumerate(meta["canonicalize_commands"], 1):
            run(f"canonicalize.{slot}", command)
    if mode in {"fix", "canon"}:
        return

    run("fmt", meta["format_command"])
    run("clippy", meta["clippy_command"])
    run("test", meta["test_command"])
    if mode == "deep":
        run("doc", meta["doc_command"])


if __name__ == "__main__":
    main()
