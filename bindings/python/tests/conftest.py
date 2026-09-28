"""Shared fixtures for the brix-client pytest suite.

Locates the `brix` binary via the `BRIX_BIN` environment variable, falling
back to `target/debug/brix` relative to the repository root, and skips the
whole suite if neither exists (so `pytest` run without a prior `cargo build`
fails clearly rather than with a confusing subprocess error).
"""

import os
from pathlib import Path

import pytest


def repo_root() -> Path:
    # bindings/python/tests/conftest.py -> repo root is three parents up.
    return Path(__file__).resolve().parents[3]


def resolve_brix_bin() -> Path:
    env_bin = os.environ.get("BRIX_BIN")
    if env_bin:
        return Path(env_bin)
    return repo_root() / "target" / "debug" / "brix"


@pytest.fixture(scope="session")
def brix_bin() -> str:
    bin_path = resolve_brix_bin()
    if not bin_path.exists():
        pytest.skip(
            f"brix binary not found at '{bin_path}' (set BRIX_BIN or run "
            "`cargo build -p brix-cli` first)"
        )
    return str(bin_path)


@pytest.fixture(scope="session")
def examples_dir() -> Path:
    return repo_root()
