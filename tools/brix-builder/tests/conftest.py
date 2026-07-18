from __future__ import annotations

from pathlib import Path

import pytest

from brix_builder.config import BuilderConfig


@pytest.fixture
def package_root(tmp_path: Path) -> Path:
    (tmp_path / "src").mkdir()
    (tmp_path / "brix.toml").write_text(
        '[package]\nname = "demo.orders"\nversion = "0.1.0"\nauthors = []\n\n[dependencies]\n',
        encoding="utf-8",
    )
    (tmp_path / "src" / "world.brix").write_text(
        "package demo.orders @ 0.1.0\nentity Order { key id: String }\n",
        encoding="utf-8",
    )
    return tmp_path


@pytest.fixture
def fake_brix(tmp_path: Path) -> Path:
    path = tmp_path / "fake-brix"
    path.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        "verb = sys.argv[1] if len(sys.argv) > 1 else ''\n"
        "if verb == '--help':\n"
        "    print('brix check <path>\\nbrix fmt <path>\\nbrix test <path>\\nbrix quality <path>')\n"
        "    raise SystemExit(0)\n"
        "if verb in {'check', 'fmt', 'test', 'quality', 'build'}:\n"
        "    print('ok ' + verb)\n"
        "    raise SystemExit(0)\n"
        "raise SystemExit(2)\n",
        encoding="utf-8",
    )
    path.chmod(0o755)
    return path


@pytest.fixture
def config(package_root: Path, fake_brix: Path) -> BuilderConfig:
    return BuilderConfig(root=package_root, brix_binary=fake_brix, max_actions=8)
