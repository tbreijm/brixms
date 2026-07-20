from __future__ import annotations

from pathlib import Path

from brix_builder.actions import ProposePatchAction
from brix_builder.config import BuilderConfig
from brix_builder.tools import BrixTools, ToolResult


def proposal() -> ProposePatchAction:
    return ProposePatchAction.model_validate(
        {
            "action": "propose_patch",
            "files": [
                {
                    "path": "src/world.brix",
                    "content": (
                        "package demo.orders @ 0.1.0\n"
                        "entity Order { key id: String }\n"
                        "query AllOrders() -> Rel<{ id: String }> from { Order(id) }\n"
                    ),
                }
            ],
            "expected_change": {"adds": ["query AllOrders"]},
            "required_validation": ["check", "package_build"],
            "reason": "add requested query",
        }
    )


def test_candidate_is_in_memory_and_compiler_tools_use_a_temporary_tree(
    config: BuilderConfig,
) -> None:
    tools = BrixTools(config)
    live_before = (config.root / "src" / "world.brix").read_text(encoding="utf-8")

    result = tools.dispatch(proposal())
    assert result.ok
    assert "AllOrders" in tools.candidate.diff()
    assert tools.check_candidate().ok
    assert tools.package_build().ok
    assert (config.root / "src" / "world.brix").read_text(
        encoding="utf-8"
    ) == live_before


def test_allowlist_rejects_non_package_code(config: BuilderConfig) -> None:
    action = proposal().model_copy(
        update={
            "files": [
                proposal().files[0].model_copy(update={"path": "runtime/agent.py"})
            ]
        }
    )
    result = BrixTools(config).dispatch(action)
    assert not result.ok
    assert result.status == "rejected"


def test_fail_closed_test_and_quality_diagnostics_are_unavailable() -> None:
    for capability, code in (
        ("brix test", "BRX-TEST-0001"),
        ("brix quality", "BRX-QUALITY-0001"),
    ):
        raw = ToolResult(
            False,
            "failed",
            data={"exit_code": 1, "stdout": f'{{"code":"{code}"}}', "stderr": ""},
        )
        result = BrixTools._classify_unimplemented(raw, capability)
        assert not result.ok
        assert result.status == "unavailable"
        assert capability in result.message


def test_exact_edit_changes_only_the_candidate(config: BuilderConfig) -> None:
    tools = BrixTools(config)
    action = ProposePatchAction.model_validate(
        {
            "action": "propose_patch",
            "edits": [
                {
                    "path": "src/world.brix",
                    "old_text": "entity Order { key id: String }",
                    "new_text": "entity Order { key id: String; note: String }",
                }
            ],
            "expected_change": {"adds": ["Order.note"]},
            "required_validation": ["check"],
            "reason": "bounded existing-file change",
        }
    )

    result = tools.dispatch(action)
    assert result.ok
    assert result.data["exact_edits"] == 1
    assert "note: String" in tools.candidate.diff()
    assert "note: String" not in (config.root / "src" / "world.brix").read_text()


def test_format_gate_host_applies_canonical_fmt_when_check_passes(
    package_root: Path, tmp_path: Path
) -> None:
    """Host owns whitespace once the candidate typechecks — no model turn needed."""

    binary = tmp_path / "fmt-aware-brix"
    binary.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        "\n"
        "def resolve(path: pathlib.Path) -> pathlib.Path:\n"
        "    if path.is_dir():\n"
        "        return path / 'src' / 'world.brix'\n"
        "    return path\n"
        "\n"
        "def canonical(path: pathlib.Path) -> str:\n"
        "    text = path.read_text(encoding='utf-8')\n"
        "    while '\\n\\n\\n' in text:\n"
        "        text = text.replace('\\n\\n\\n', '\\n\\n')\n"
        "    return text\n"
        "\n"
        "verb = sys.argv[1]\n"
        "path = resolve(pathlib.Path(sys.argv[2]))\n"
        "if verb == 'check':\n"
        "    raise SystemExit(0)\n"
        "if verb == 'fmt':\n"
        "    rendered = canonical(path)\n"
        "    if '--check' in sys.argv:\n"
        "        raise SystemExit(0 if path.read_text(encoding='utf-8') == rendered else 1)\n"
        "    if '--write' in sys.argv:\n"
        "        path.write_text(rendered, encoding='utf-8')\n"
        "        print(f'brix: formatted {path}')\n"
        "        raise SystemExit(0)\n"
        "    sys.stdout.write(rendered)\n"
        "    raise SystemExit(0)\n"
        "raise SystemExit(2)\n",
        encoding="utf-8",
    )
    binary.chmod(0o755)
    tools = BrixTools(BuilderConfig(root=package_root, brix_binary=binary))
    messy = (
        "package demo.orders @ 0.1.0\n"
        "entity Order { key id: String }\n"
        "\n\n\n"
        "fn noteOf(id: String) -> String = id\n"
    )
    tools.candidate.overlay["src/world.brix"] = messy
    result = tools.format_candidate()
    assert result.ok, result.message
    assert result.status == "canonical"
    assert result.data.get("host_applied_fmt") is True
    assert "\n\n\n" not in tools.candidate.contents()["src/world.brix"]
    assert "fn noteOf" in tools.candidate.contents()["src/world.brix"]


def _multi_file_fmt_binary(tmp_path: Path) -> Path:
    """A fake `brix` whose `fmt`/`check` operate on *every* local `.brix`
    file under the resolved package root's `src/` — like the real multi-file
    (issue #42) CLI does — so a `fmt <path> --write` call rewrites each file
    at its own real path in one pass, never conflating one file's rendered
    output with another's."""

    binary = tmp_path / "multi-file-brix"
    binary.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        "\n"
        "def pkg_root(path: pathlib.Path) -> pathlib.Path:\n"
        "    return path if path.is_dir() else path.parent.parent\n"
        "\n"
        "def canonical(text: str) -> str:\n"
        "    while '\\n\\n\\n' in text:\n"
        "        text = text.replace('\\n\\n\\n', '\\n\\n')\n"
        "    return text\n"
        "\n"
        "verb = sys.argv[1]\n"
        "root = pkg_root(pathlib.Path(sys.argv[2]))\n"
        "sources = sorted((root / 'src').glob('*.brix'))\n"
        "if verb == 'check':\n"
        "    raise SystemExit(0)\n"
        "if verb == 'fmt':\n"
        "    if '--check' in sys.argv:\n"
        "        ok = all(\n"
        "            canonical(p.read_text(encoding='utf-8')) == p.read_text(encoding='utf-8')\n"
        "            for p in sources\n"
        "        )\n"
        "        raise SystemExit(0 if ok else 1)\n"
        "    if '--write' in sys.argv:\n"
        "        for p in sources:\n"
        "            current = p.read_text(encoding='utf-8')\n"
        "            rendered = canonical(current)\n"
        "            if rendered != current:\n"
        "                p.write_text(rendered, encoding='utf-8')\n"
        "                print(f'brix: formatted {p}')\n"
        "        raise SystemExit(0)\n"
        "    for p in sources:\n"
        "        sys.stdout.write(canonical(p.read_text(encoding='utf-8')))\n"
        "    raise SystemExit(0)\n"
        "raise SystemExit(2)\n",
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return binary


def test_format_gate_covers_every_local_file_not_just_world_brix(
    package_root: Path, tmp_path: Path
) -> None:
    """Issue #42: a non-canonical submodule must surface in — and be fixed
    by — the format gate exactly like a non-canonical `world.brix` always
    has, not be silently skipped because the gate only ever looked at one
    file."""

    binary = _multi_file_fmt_binary(tmp_path)
    tools = BrixTools(BuilderConfig(root=package_root, brix_binary=binary))
    canonical_world = (package_root / "src" / "world.brix").read_text(encoding="utf-8")
    tools.candidate.overlay["src/ops.brix"] = (
        "fn scale(x: Int) -> Int = x + x\n\n\n\nfn double(x: Int) -> Int = x + x\n"
    )

    result = tools.format_candidate()

    assert result.ok, result.message
    assert result.status == "canonical"
    assert result.data.get("host_applied_fmt") is True
    contents = tools.candidate.contents()
    assert "\n\n\n" not in contents["src/ops.brix"]
    assert "fn double" in contents["src/ops.brix"]
    # world.brix was already canonical and untouched by the submodule fix.
    assert contents["src/world.brix"] == canonical_world
