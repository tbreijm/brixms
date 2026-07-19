"""Narrow BrixMS tools executed against an isolated candidate package."""

from __future__ import annotations

import difflib
import hashlib
import json
import re
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any

from .actions import (
    Action,
    CheckCandidateAction,
    DiffCandidateAction,
    FindAction,
    FormatCandidateAction,
    ImpactCandidateAction,
    InspectAction,
    PackageBuildAction,
    ProjectContextAction,
    ProposePatchAction,
    QualityCandidateAction,
    ReadSourceAction,
    TestCandidateAction,
)
from .config import BuilderConfig


DECLARATION = re.compile(
    r"^\s*(?P<kind>entity|rel|state\s+rel|event\s+rel|open\s+rel|derive|constraint|"
    r"query|protocol|scenario|fn|type|enum|record|brick|trait|impl)\s+(?P<name>[A-Za-z_][\w.]*)",
    re.MULTILINE,
)
ALLOWED_EXACT = {"brix.toml", "brix.lock", "OWNER.md", "README.md"}
IGNORED_PARTS = {".git", ".brix-cache", "target", ".venv", "__pycache__"}


@dataclass(frozen=True)
class ToolResult:
    ok: bool
    status: str
    data: dict[str, Any] = field(default_factory=dict)
    message: str = ""

    def model_payload(self) -> str:
        return json.dumps(
            {
                "ok": self.ok,
                "status": self.status,
                "message": self.message,
                "data": self.data,
            },
            sort_keys=True,
            ensure_ascii=False,
        )


class ToolError(RuntimeError):
    pass


class CandidatePackage:
    """An in-memory overlay that is materialized only into temporary trees."""

    def __init__(self, root: Path):
        self.root = root.resolve()
        if not self.root.is_dir():
            raise ToolError(f"package root is not a directory: {self.root}")
        self.base = self._snapshot()
        self.overlay: dict[str, str] = {}
        self.expected_change: dict[str, Any] = {}
        self.required_validation: list[str] = []

    def _snapshot(self) -> dict[str, str]:
        files: dict[str, str] = {}
        for path in sorted(self.root.rglob("*")):
            if not path.is_file() or any(part in IGNORED_PARTS for part in path.parts):
                continue
            rel = path.relative_to(self.root).as_posix()
            if self.allowed(rel):
                try:
                    files[rel] = path.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    continue
        return files

    @staticmethod
    def allowed(rel: str) -> bool:
        path = PurePosixPath(rel)
        if (
            path.is_absolute()
            or ".." in path.parts
            or any(part.startswith(".") for part in path.parts)
        ):
            return False
        return path.suffix == ".brix" or path.name in ALLOWED_EXACT

    def propose(self, action: ProposePatchAction) -> ToolResult:
        proposed: dict[str, str] = {}
        for candidate in action.files:
            rel = PurePosixPath(candidate.path).as_posix()
            if not self.allowed(rel):
                return ToolResult(
                    False,
                    "rejected",
                    message=f"path is outside the BrixMS package allowlist: {rel}",
                )
            proposed[rel] = candidate.content

        staged = self.contents() | proposed
        for edit in action.edits:
            rel = PurePosixPath(edit.path).as_posix()
            if not self.allowed(rel):
                return ToolResult(
                    False,
                    "rejected",
                    message=f"path is outside the BrixMS package allowlist: {rel}",
                )
            current = staged.get(rel)
            if current is None:
                return ToolResult(
                    False,
                    "rejected",
                    message=f"exact edit target does not exist: {rel}",
                )
            occurrences = current.count(edit.old_text)
            if occurrences != 1:
                return ToolResult(
                    False,
                    "rejected",
                    message=(
                        f"exact edit anchor must occur once in {rel}; found {occurrences}"
                    ),
                )
            staged[rel] = current.replace(edit.old_text, edit.new_text, 1)
            proposed[rel] = staged[rel]
        self.overlay.update(proposed)
        self.expected_change = action.expected_change.model_dump()
        self.required_validation = list(dict.fromkeys(action.required_validation))
        return ToolResult(
            True,
            "candidate_recorded",
            data={
                "files": sorted(proposed),
                "exact_edits": len(action.edits),
                "required_validation": self.required_validation,
                "diff": self.diff(),
            },
            message="candidate recorded in memory; live files were not modified",
        )

    def contents(self) -> dict[str, str]:
        return self.base | self.overlay

    def diff(self) -> str:
        chunks: list[str] = []
        for rel in sorted(set(self.base) | set(self.overlay)):
            before = self.base.get(rel, "")
            after = self.overlay.get(rel, before)
            if before == after:
                continue
            chunks.extend(
                difflib.unified_diff(
                    before.splitlines(keepends=True),
                    after.splitlines(keepends=True),
                    fromfile=f"a/{rel}",
                    tofile=f"b/{rel}",
                )
            )
        return "".join(chunks)

    def revision(self) -> str:
        digest = hashlib.sha256()
        for rel, content in sorted(self.contents().items()):
            digest.update(rel.encode())
            digest.update(b"\0")
            digest.update(content.encode())
            digest.update(b"\0")
        return digest.hexdigest()

    def materialize(self, destination: Path) -> None:
        for rel, content in self.contents().items():
            path = destination / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")

    def entry_operand(self, root: Path) -> Path:
        if (root / "brix.toml").exists() and (root / "src" / "world.brix").exists():
            return root
        if (root / "src" / "world.brix").exists():
            return root / "src" / "world.brix"
        sources = sorted(root.rglob("*.brix"))
        if len(sources) == 1:
            return sources[0]
        raise ToolError(
            "cannot select a package entry; expected brix.toml + src/world.brix or one .brix file"
        )


class BrixTools:
    def __init__(self, config: BuilderConfig):
        self.config = config.normalized()
        self.candidate = CandidatePackage(self.config.root)

    def dispatch(self, action: Action) -> ToolResult:
        if isinstance(action, ProjectContextAction):
            return self.project_context()
        if isinstance(action, FindAction):
            return self.find(action)
        if isinstance(action, InspectAction):
            return self.inspect(action)
        if isinstance(action, ReadSourceAction):
            return self.read_source(action)
        if isinstance(action, ProposePatchAction):
            return self.candidate.propose(action)
        if isinstance(action, CheckCandidateAction):
            return self.check_candidate()
        if isinstance(action, FormatCandidateAction):
            return self.format_candidate()
        if isinstance(action, TestCandidateAction):
            return self.test_candidate(action)
        if isinstance(action, QualityCandidateAction):
            return self.quality_candidate(action)
        if isinstance(action, DiffCandidateAction):
            return self.diff_candidate()
        if isinstance(action, ImpactCandidateAction):
            return self.impact_candidate()
        if isinstance(action, PackageBuildAction):
            return self.package_build()
        return ToolResult(
            False, "rejected", message=f"action is not a tool: {action.action}"
        )

    def project_context(self) -> ToolResult:
        manifest = self.candidate.contents().get("brix.toml")
        package: dict[str, str] = {}
        if manifest:
            for key in ("name", "version"):
                found = re.search(
                    rf"^\s*{key}\s*=\s*\"([^\"]+)\"", manifest, re.MULTILINE
                )
                if found:
                    package[key] = found.group(1)
        declarations = self._declarations()
        return ToolResult(
            True,
            "ok",
            data={
                "language_edition": "BrixMS v9",
                "package": package,
                "program_revision": self.candidate.revision(),
                "candidate_active": bool(self.candidate.overlay),
                "files": sorted(self.candidate.contents()),
                "declaration_counts": self._kind_counts(declarations),
                "compiler": str(self.config.brix_binary),
            },
        )

    def find(self, action: FindAction) -> ToolResult:
        query = action.query.casefold()
        kinds = {kind.casefold() for kind in action.kinds}
        matches = []
        for decl in self._declarations():
            if kinds and decl["kind"].casefold() not in kinds:
                continue
            if (
                query not in decl["name"].casefold()
                and query not in decl["source"].casefold()
            ):
                continue
            matches.append(decl)
            if len(matches) >= action.limit:
                break
        return ToolResult(True, "ok", data={"matches": matches})

    def inspect(self, action: InspectAction) -> ToolResult:
        declarations = self._declarations()
        found = []
        missing = []
        for subject in action.subjects:
            short = subject.rsplit(".", 1)[-1]
            hits = [item for item in declarations if item["name"] in {subject, short}]
            if hits:
                found.extend(hits)
            else:
                missing.append(subject)
        check = self.check_candidate()
        return ToolResult(
            check.ok,
            "partial_semantic_inspection" if check.ok else "compiler_rejected",
            data={
                "declarations": found,
                "missing": missing,
                "compiler_check": check.data,
                "resolution_limit": (
                    "the current public compiler does not yet export resolved ownership, visibility, "
                    "effect, and dependency facts; source shape plus compiler verdict is returned"
                ),
            },
            message=check.message,
        )

    def read_source(self, action: ReadSourceAction) -> ToolResult:
        rel = PurePosixPath(action.path).as_posix()
        if not self.candidate.allowed(rel):
            return ToolResult(
                False,
                "rejected",
                message="source path is outside the package allowlist",
            )
        content = self.candidate.contents().get(rel)
        if content is None:
            return ToolResult(
                False, "not_found", message=f"no candidate source at {rel}"
            )
        lines = content.splitlines()
        end = min(action.end_line, len(lines))
        if action.start_line > end:
            return ToolResult(
                False, "invalid_range", message="requested source range is empty"
            )
        numbered = [
            f"{number}: {lines[number - 1]}"
            for number in range(action.start_line, end + 1)
        ]
        return ToolResult(
            True,
            "ok",
            data={
                "path": rel,
                "start": action.start_line,
                "end": end,
                "content": "\n".join(lines[action.start_line - 1 : end]),
                "text": "\n".join(numbered),
            },
        )

    def bootstrap_context(self, brief: str) -> dict[str, Any]:
        """Small deterministic retrieval slice injected before the first action."""

        declarations = self._declarations()
        lowered_brief = brief.casefold()
        selected = [
            item
            for item in declarations
            if item["name"].casefold() in lowered_brief or item["kind"] == "query"
        ]
        if not selected:
            selected = declarations[:20]

        snippets: list[dict[str, Any]] = []
        by_file: dict[str, set[int]] = {}
        for item in selected[:20]:
            lines = by_file.setdefault(item["path"], set())
            lines.update(range(max(1, item["line"] - 2), item["line"] + 15))
        for rel, wanted in by_file.items():
            content = self.candidate.contents().get(rel, "")
            lines = content.splitlines()
            ranges: list[tuple[int, int]] = []
            for number in sorted(n for n in wanted if n <= len(lines)):
                if ranges and number <= ranges[-1][1] + 1:
                    ranges[-1] = (ranges[-1][0], number)
                else:
                    ranges.append((number, number))
            for start, end in ranges:
                snippets.append(
                    {
                        "path": rel,
                        "start_line": start,
                        "end_line": end,
                        "content": "\n".join(lines[start - 1 : end]),
                    }
                )
        return {
            "project": self.project_context().data,
            "declarations": declarations[:100],
            "relevant_source": snippets[:12],
        }

    def check_candidate(self) -> ToolResult:
        return self._run_brix("check", "--diagnostic-format", "json")

    def format_candidate(self) -> ToolResult:
        with tempfile.TemporaryDirectory(prefix="brix-builder-format-") as temporary:
            root = Path(temporary)
            self.candidate.materialize(root)
            try:
                operand = self.candidate.entry_operand(root)
            except ToolError as error:
                return ToolResult(False, "configuration_error", message=str(error))
            result = self._command(["fmt", str(operand), "--check"])
            if result.ok:
                return ToolResult(
                    True,
                    "canonical",
                    data={**result.data, "diff": self.candidate.diff()},
                    message="candidate is canonically formatted; no source was rewritten",
                )
            return ToolResult(
                False,
                "noncanonical",
                data={**result.data, "diff": self.candidate.diff()},
                message=(
                    "candidate is not canonically formatted; the host will not rewrite "
                    "the whole file because that could expand an unrelated diff"
                ),
            )

    def test_candidate(self, action: TestCandidateAction) -> ToolResult:
        args = ["test"]
        args.extend(action.selectors)
        args.extend(("--diagnostic-format", "json"))
        result = self._run_brix(*args)
        return self._classify_unimplemented(result, "brix test")

    def quality_candidate(self, action: QualityCandidateAction) -> ToolResult:
        result = self._run_brix(
            "quality",
            "--profile",
            action.profile,
            "--diagnostic-format",
            "json",
        )
        return self._classify_unimplemented(result, "brix quality")

    def diff_candidate(self) -> ToolResult:
        changed = []
        for rel, content in sorted(self.candidate.overlay.items()):
            if self.candidate.base.get(rel) != content:
                changed.append(rel)
        return ToolResult(
            False,
            "partial",
            data={
                "files": changed,
                "diff": self.candidate.diff(),
                "expected_change": self.candidate.expected_change,
                "semantic_diff_available": False,
            },
            message="textual diff is available, but the mandatory compiler semantic-diff oracle is not",
        )

    def impact_candidate(self) -> ToolResult:
        before = self._declarations(self.candidate.base)
        after = self._declarations(self.candidate.contents())
        before_names = {(item["kind"], item["name"]) for item in before}
        after_names = {(item["kind"], item["name"]) for item in after}
        added = sorted(after_names - before_names)
        removed = sorted(before_names - after_names)
        changed_names = {name for _, name in added + removed}
        references = []
        for rel, content in self.candidate.contents().items():
            for name in changed_names:
                count = len(re.findall(rf"\b{re.escape(name)}\b", content))
                if count:
                    references.append(
                        {"path": rel, "subject": name, "occurrences": count}
                    )
        return ToolResult(
            True,
            "lexical_impact_only",
            data={
                "added": added,
                "removed": removed,
                "references": references,
                "graph_impact_available": False,
            },
            message="resolved dependency graph export is not available in the public compiler yet",
        )

    def package_build(self) -> ToolResult:
        return self._run_brix("build", "--diagnostic-format", "json")

    def _run_brix(self, *args: str) -> ToolResult:
        with tempfile.TemporaryDirectory(prefix="brix-builder-candidate-") as temporary:
            root = Path(temporary)
            self.candidate.materialize(root)
            try:
                operand = self.candidate.entry_operand(root)
            except ToolError as error:
                return ToolResult(False, "configuration_error", message=str(error))
            return self._command([args[0], str(operand), *args[1:]])

    def _command(self, args: list[str]) -> ToolResult:
        binary = self.config.brix_binary
        if not binary.is_file():
            return ToolResult(
                False,
                "compiler_unavailable",
                message=f"brix binary not found: {binary}",
            )
        try:
            completed = subprocess.run(
                [str(binary), *args],
                cwd=self.config.root,
                capture_output=True,
                text=True,
                timeout=self.config.request_timeout_seconds,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return ToolResult(
                False, "timeout", message=f"brix {' '.join(args)} timed out"
            )
        stdout = completed.stdout[-24000:]
        stderr = completed.stderr[-24000:]
        return ToolResult(
            completed.returncode == 0,
            "passed" if completed.returncode == 0 else "failed",
            data={
                "exit_code": completed.returncode,
                "stdout": stdout,
                "stderr": stderr,
            },
        )

    @staticmethod
    def _classify_unimplemented(result: ToolResult, capability: str) -> ToolResult:
        # `BRX-TEST-0001` is the stable code for unavailable scenario semantics.
        # Quality unavailable evidence uses `BRX-QUALITY-0003`.
        combined = f"{result.data.get('stdout', '')}\n{result.data.get('stderr', '')}"
        if "not yet implemented" in combined or "BRX-TEST-0001" in combined:
            return ToolResult(
                False,
                "unavailable",
                data=result.data,
                message=f"{capability} engine is unavailable in this brix toolchain revision",
            )
        return result

    def _declarations(
        self, files: dict[str, str] | None = None
    ) -> list[dict[str, Any]]:
        declarations = []
        for rel, content in sorted((files or self.candidate.contents()).items()):
            if not rel.endswith(".brix"):
                continue
            lines = content.splitlines()
            for match in DECLARATION.finditer(content):
                line = content.count("\n", 0, match.start()) + 1
                declarations.append(
                    {
                        "kind": " ".join(match.group("kind").split()),
                        "name": match.group("name"),
                        "path": rel,
                        "line": line,
                        "source": lines[line - 1].strip(),
                    }
                )
        return declarations

    @staticmethod
    def _kind_counts(declarations: list[dict[str, Any]]) -> dict[str, int]:
        counts: dict[str, int] = {}
        for declaration in declarations:
            kind = declaration["kind"]
            counts[kind] = counts.get(kind, 0) + 1
        return counts
