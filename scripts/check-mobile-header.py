#!/usr/bin/env python3
"""Check the C header using the exact Agent Foundation resolved by Cargo.lock."""

from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    manifest = tomllib.loads((ROOT / "crates/mobile-ffi/Cargo.toml").read_text())
    dependency = manifest["dependencies"]["sarmg-mobile-ffi"]
    revision = dependency["rev"]
    version = dependency["version"].removeprefix("=")
    expected_source = f"git+{dependency['git']}?rev={revision}#{revision}"
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT,
    ))
    matches = [package for package in metadata["packages"]
               if package["name"] == "sarmg-mobile-ffi"
               and package["version"] == version
               and package["source"] == expected_source]
    if len(matches) != 1:
        raise SystemExit("expected one exact, locked Agent Foundation FFI dependency")
    foundation = Path(matches[0]["manifest_path"]).resolve().parents[3]
    subprocess.run([
        sys.executable, str(foundation / "tools/ffi_header.py"),
        "--product-source", "crates/mobile-ffi/src/lib.rs",
        "--guard", "MEDIA_BACKUP_FFI_V2_H",
        "--check", "crates/mobile-ffi/include/media_backup_ffi_v2.h",
    ], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
