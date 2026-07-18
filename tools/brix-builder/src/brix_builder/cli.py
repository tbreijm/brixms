"""Codex-style terminal entry point and local Unix-socket sidecar."""

from __future__ import annotations

import argparse
import importlib.util
import json
import socket
import stat
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path

from .actions import action_json_schema
from .agent import BrixBuilderTeam, TeamResult
from .config import BuilderConfig
from .model import DirectMlxBackend, LocalServerBackend, ModelBackend, ModelError


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="brix-builder", description="Local compiler-grounded BrixMS package team"
    )
    result.add_argument(
        "--root", type=Path, default=Path.cwd(), help="BrixMS package root"
    )
    result.add_argument(
        "--brix",
        type=Path,
        default=Path("target/debug/brix"),
        help="path to the brix compiler CLI",
    )
    result.add_argument("--model", default="mlx-community/Qwen3.5-4B-MLX-4bit")
    result.add_argument("--adapter", default=None)
    result.add_argument("--backend", choices=("mlx", "server"), default="mlx")
    result.add_argument("--endpoint", default="http://127.0.0.1:8080/v1")
    result.add_argument("--context", type=int, default=8192)
    result.add_argument("--max-actions", type=int, default=12)
    result.add_argument("--repair-rounds", type=int, default=3)
    commands = result.add_subparsers(dest="command", required=True)

    run = commands.add_parser("run", help="run one coder–tester–reviewer task")
    run.add_argument("brief", nargs="+", help="package task")
    run.add_argument("--json", action="store_true", dest="as_json")

    commands.add_parser("chat", help="interactive local package-building session")
    commands.add_parser("doctor", help="check local compiler/runtime readiness")
    commands.add_parser("schema", help="print the typed action JSON Schema")
    serve = commands.add_parser(
        "serve", help="serve NDJSON tasks over a local Unix socket"
    )
    serve.add_argument("--socket", type=Path, required=True)
    return result


def config_from(args: argparse.Namespace) -> BuilderConfig:
    return BuilderConfig(
        root=args.root,
        brix_binary=args.brix,
        model=args.model,
        endpoint=args.endpoint,
        context_tokens=args.context,
        max_actions=args.max_actions,
        repair_rounds=args.repair_rounds,
    ).normalized()


def backend_from(args: argparse.Namespace, config: BuilderConfig) -> ModelBackend:
    if args.backend == "server":
        return LocalServerBackend(
            config.endpoint, config.model, config.request_timeout_seconds
        )
    return DirectMlxBackend(config.model, args.adapter)


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    config = config_from(args)
    if args.command == "schema":
        print(json.dumps(action_json_schema(), indent=2, sort_keys=True))
        return 0
    if args.command == "doctor":
        return doctor(config, args.backend)
    try:
        model = backend_from(args, config)
        if args.command == "run":
            result = BrixBuilderTeam(config, model).run(" ".join(args.brief))
            print_result(result, args.as_json)
            return 0 if result.status == "validated_candidate" else 1
        if args.command == "chat":
            return chat(config, model)
        if args.command == "serve":
            return serve(config, model, args.socket)
    except (ModelError, OSError, RuntimeError) as error:
        print(f"brix-builder: {error}", file=sys.stderr)
        return 2
    return 2


def doctor(config: BuilderConfig, backend: str) -> int:
    checks: list[tuple[str, bool, str]] = []
    if config.brix_binary.is_file():
        completed = subprocess.run(
            [str(config.brix_binary), "--help"],
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
        output = completed.stdout
        checks.append(
            ("brix compiler", completed.returncode == 0, str(config.brix_binary))
        )
        checks.append(
            (
                "brix check",
                "brix check <path>" in output,
                "public static/semantic oracle",
            )
        )
        checks.append(("brix fmt", "brix fmt <path>" in output, "canonical formatter"))
        checks.append(
            (
                "brix test command",
                "brix test <path>" in output,
                "public fail-closed gate; engine availability is reported per run",
            )
        )
        checks.append(
            (
                "brix quality command",
                "brix quality <path>" in output,
                "public fail-closed gate; engine availability is reported per run",
            )
        )
    else:
        checks.append(("brix compiler", False, f"not found: {config.brix_binary}"))
    if backend == "mlx":
        if importlib.util.find_spec("mlx_lm") is not None:
            checks.append(("MLX LM", True, "in-process backend installed"))
        else:
            checks.append(("MLX LM", False, "install with pip install -e '.[mlx]'"))
    else:
        checks.append(("local model server", True, config.endpoint))

    for name, ok, detail in checks:
        print(f"{'PASS' if ok else 'MISS'}  {name}: {detail}")
    return 0 if all(ok for _, ok, _ in checks) else 1


def chat(config: BuilderConfig, model: ModelBackend) -> int:
    print(
        "BrixBuilder-4B local package team. Candidate patches are never auto-applied."
    )
    print("Enter a package task, or /quit.")
    while True:
        try:
            brief = input("brix-builder> ").strip()
        except EOFError:
            print()
            return 0
        if brief in {"/quit", "/exit"}:
            return 0
        if not brief:
            continue
        result = BrixBuilderTeam(config, model).run(brief)
        print_result(result, False)


def serve(config: BuilderConfig, model: ModelBackend, socket_path: Path) -> int:
    path = socket_path.expanduser().resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        mode = path.stat().st_mode
        if not stat.S_ISSOCK(mode):
            raise RuntimeError(f"refusing to replace non-socket path: {path}")
        path.unlink()
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        server.bind(str(path))
        path.chmod(0o600)
        server.listen(8)
        print(f"brix-builder: listening on {path}")
        while True:
            connection, _ = server.accept()
            with connection, connection.makefile("rwb") as stream:
                line = stream.readline()
                try:
                    request = json.loads(line)
                    brief = request["brief"]
                    if not isinstance(brief, str) or not brief.strip():
                        raise ValueError("brief must be a non-empty string")
                    result = BrixBuilderTeam(config, model).run(brief)
                    response = {"ok": True, "result": asdict(result)}
                except (json.JSONDecodeError, KeyError, ValueError, TypeError) as error:
                    response = {"ok": False, "error": str(error)}
                stream.write(json.dumps(response, default=str).encode() + b"\n")
                stream.flush()
    except KeyboardInterrupt:
        return 0
    finally:
        server.close()
        if path.exists() and stat.S_ISSOCK(path.stat().st_mode):
            path.unlink()


def print_result(result: TeamResult, as_json: bool) -> None:
    if as_json:
        print(json.dumps(asdict(result), indent=2, default=str))
        return
    print(f"\nPackage status: {result.status}")
    print(result.summary)
    print("Gates:")
    for gate, status in result.gates.items():
        print(f"  {gate}: {status}")
    if result.residual_obligations:
        print("Unresolved:")
        for item in result.residual_obligations:
            print(f"  - {item}")
    print("Proposed patch (not applied):")
    print(result.diff or "  (no changes)")


if __name__ == "__main__":
    raise SystemExit(main())
