"""Codex-style terminal entry point and local Unix-socket sidecar."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import socket
import stat
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any

from .actions import action_json_schema
from .agent import BrixBuilderTeam, TeamResult
from .config import BuilderConfig
from .model import DirectMlxBackend, LocalServerBackend, ModelBackend, ModelError
from .tickets import TicketState, TicketStore, TicketWorker, WorkerLock, _pid_is_alive


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
    result.add_argument("--critic-model", default=None)
    result.add_argument("--critic-adapter", default=None)
    result.add_argument("--backend", choices=("mlx", "server"), default="mlx")
    result.add_argument("--endpoint", default="http://127.0.0.1:8080/v1")
    result.add_argument("--critic-endpoint", default=None)
    result.add_argument(
        "--queue",
        type=Path,
        default=_default_queue(),
        help="durable worker state (defaults outside the source checkout)",
    )
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

    enqueue = commands.add_parser("enqueue", help="add a scoped durable package ticket")
    enqueue.add_argument(
        "brief", nargs="*", help="package task; omit when --from-file supplies one"
    )
    enqueue.add_argument("--ticket-id", default=None)
    enqueue.add_argument("--package", default=None, dest="package_path")
    enqueue.add_argument("--allow-file", action="append", dest="write_allowlist")
    enqueue.add_argument(
        "--gate",
        action="append",
        choices=(
            "format",
            "check",
            "test",
            "quality",
            "diff",
            "impact",
            "package_build",
        ),
        dest="acceptance_gates",
    )
    enqueue.add_argument("--max-iterations", type=int, default=None)
    enqueue.add_argument(
        "--from-file",
        type=Path,
        default=None,
        dest="from_file",
        help=(
            "load a JSON ticket spec (brief, ticket_id, package_path, "
            "write_allowlist, acceptance_gates, max_iterations, "
            "max_actions_per_role, context_tokens, metadata); see "
            "tickets/seeds/ for BrixMS core-package examples. Explicit "
            "flags on the command line override the file's fields."
        ),
    )

    commands.add_parser("tickets", help="list durable tickets")
    commands.add_parser(
        "status", help="show queue root, worker lock, and ticket counts"
    )
    commands.add_parser(
        "reclaim", help="requeue any 'running' tickets abandoned by a dead worker"
    )
    inspect = commands.add_parser("inspect-ticket", help="show one ticket state")
    inspect.add_argument("ticket_id")
    run_ticket = commands.add_parser(
        "run-ticket", help="run a ticket to a terminal state"
    )
    run_ticket.add_argument("ticket_id")
    run_ticket.add_argument("--one-iteration", action="store_true")
    loop = commands.add_parser(
        "loop", help="process queued tickets until the queue is empty"
    )
    loop.add_argument(
        "--once", action="store_true", help="process at most one iteration"
    )
    resume = commands.add_parser("resume", help="resume an interrupted ticket")
    resume.add_argument("ticket_id")
    cancel = commands.add_parser(
        "cancel", help="cancel a ticket without touching source"
    )
    cancel.add_argument("ticket_id")
    cancel.add_argument("reason")
    export = commands.add_parser(
        "export-proposal", help="export an inert patch and evidence bundle"
    )
    export.add_argument("ticket_id")
    export.add_argument("destination", type=Path)
    return result


def _default_queue() -> Path:
    state_home = Path(
        os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state")
    )
    return state_home / "brix-builder"


def enqueue_kwargs(args: argparse.Namespace) -> dict[str, Any]:
    """Merge a `--from-file` seed ticket spec with explicit CLI overrides.

    File fields are the baseline (letting `tickets/seeds/*.json` express a
    complete ticket); any flag the operator actually passed on the command
    line -- ticket id, package, write allowlist, gates, iteration budget --
    wins over the file's value.
    """

    payload: dict[str, Any] = {}
    if args.from_file is not None:
        loaded = json.loads(args.from_file.read_text(encoding="utf-8"))
        if not isinstance(loaded, dict):
            raise ValueError(
                f"--from-file must contain a single JSON object: {args.from_file}"
            )
        payload = loaded

    brief = " ".join(args.brief) if args.brief else payload.get("brief")
    if not brief or not isinstance(brief, str):
        raise ValueError(
            "enqueue requires a brief, either as an argument or in --from-file"
        )

    return {
        "brief": brief,
        "ticket_id": args.ticket_id or payload.get("ticket_id"),
        "package_path": args.package_path or payload.get("package_path", "."),
        "write_allowlist": args.write_allowlist or payload.get("write_allowlist"),
        "acceptance_gates": args.acceptance_gates or payload.get("acceptance_gates"),
        "max_iterations": (
            args.max_iterations
            if args.max_iterations is not None
            else payload.get("max_iterations", 3)
        ),
        "max_actions_per_role": payload.get("max_actions_per_role", args.max_actions),
        "context_tokens": payload.get("context_tokens", args.context),
        "metadata": payload.get("metadata"),
    }


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


def backend_from(
    args: argparse.Namespace, config: BuilderConfig, role: str = "coder"
) -> ModelBackend:
    model = config.model if role == "coder" else (args.critic_model or config.model)
    adapter = args.adapter if role == "coder" else args.critic_adapter
    endpoint = (
        config.endpoint
        if role == "coder"
        else (args.critic_endpoint or config.endpoint)
    )
    if args.backend == "server":
        return LocalServerBackend(endpoint, model, config.request_timeout_seconds)
    return DirectMlxBackend(model, adapter)


def backends_for_roles(
    args: argparse.Namespace, config: BuilderConfig
) -> tuple[ModelBackend, ModelBackend]:
    """Coder + critic backends, sharing one loaded model when configuration matches.

    Overnight loops must not pay a second MLX load for the critic when both roles
    use the same weights. A warm `--backend server` process is even better: start
    `scripts/serve-model.sh` once, then point every worker at it.
    """

    coder = backend_from(args, config, "coder")
    same_model = (args.critic_model or config.model) == config.model
    same_adapter = args.critic_adapter == args.adapter
    same_endpoint = (args.critic_endpoint or config.endpoint) == config.endpoint
    if same_model and same_adapter and same_endpoint:
        return coder, coder
    return coder, backend_from(args, config, "critic")


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    config = config_from(args)
    if args.command == "schema":
        print(json.dumps(action_json_schema(), indent=2, sort_keys=True))
        return 0
    if args.command == "doctor":
        return doctor(config, args.backend)
    try:
        ticket_commands = {
            "enqueue",
            "tickets",
            "status",
            "reclaim",
            "inspect-ticket",
            "resume",
            "cancel",
            "export-proposal",
            "run-ticket",
            "loop",
        }
        store = (
            TicketStore(args.queue, config.root)
            if args.command in ticket_commands
            else None
        )
        if args.command == "enqueue" and store is not None:
            state = store.enqueue(**enqueue_kwargs(args))
            print_ticket(state)
            return 0
        if args.command == "tickets" and store is not None:
            print_tickets(store)
            return 0
        if args.command == "status" and store is not None:
            print_queue_status(store, config)
            return 0
        if args.command == "reclaim" and store is not None:
            reclaimed = store.reclaim_stale_running()
            if reclaimed:
                print("reclaimed abandoned running ticket(s):")
                for ticket_id in reclaimed:
                    print(f"  {ticket_id}")
            else:
                print("no abandoned running tickets found")
            return 0
        if args.command == "inspect-ticket" and store is not None:
            print(store.load(args.ticket_id).model_dump_json(indent=2))
            return 0
        if args.command == "resume" and store is not None:
            print_ticket(store.resume(args.ticket_id))
            return 0
        if args.command == "cancel" and store is not None:
            print_ticket(store.cancel(args.ticket_id, args.reason))
            return 0
        if args.command == "export-proposal" and store is not None:
            store.export(args.ticket_id, args.destination)
            print(args.destination.expanduser().resolve())
            return 0

        if args.command in {"run-ticket", "loop"} and store is not None:
            with WorkerLock(store.queue_root):
                store.reclaim_stale_running()
                coder_model, critic_model = backends_for_roles(args, config)
                worker = TicketWorker(store, config, coder_model, critic_model)
                if args.command == "run-ticket":
                    state = (
                        worker.run_iteration(args.ticket_id)
                        if args.one_iteration
                        else worker.run_to_terminal(args.ticket_id)
                    )
                    print_ticket(state)
                    return 0 if state.status == "completed" else 1
                processed = 0
                while True:
                    state = worker.run_next()
                    if state is None:
                        break
                    processed += 1
                    print_ticket(state)
                    if args.once:
                        break
                return 0 if processed else 1

        model = backend_from(args, config)
        if args.command == "run":
            result = BrixBuilderTeam(config, model).run(" ".join(args.brief))
            print_result(result, args.as_json)
            return 0 if result.status == "validated_candidate" else 1
        if args.command == "chat":
            return chat(config, model)
        if args.command == "serve":
            return serve(config, model, args.socket)
    except (KeyError, ValueError, ModelError, OSError, RuntimeError) as error:
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


def print_ticket(state: TicketState) -> None:
    print(
        f"{state.spec.id}: {state.status} "
        f"(iteration {state.iteration}/{state.spec.max_iterations})"
    )
    for obligation in state.residual_obligations:
        print(f"  - {obligation}")


def print_tickets(store: TicketStore) -> None:
    states = store.list()
    if not states:
        print(f"no tickets in this queue: {store.queue_root}")
        return
    counts: dict[str, int] = {}
    for state in states:
        counts[state.status] = counts.get(state.status, 0) + 1
    summary = ", ".join(f"{status}={count}" for status, count in sorted(counts.items()))
    print(f"{len(states)} ticket(s) in {store.queue_root}: {summary}")
    print()
    for state in states:
        brief = state.spec.brief.splitlines()[0][:70]
        gates = ",".join(state.spec.acceptance_gates)
        residual = state.residual_obligations[:2]
        residual_note = "; ".join(residual)
        if len(state.residual_obligations) > 2:
            residual_note += f" (+{len(state.residual_obligations) - 2} more)"
        print(
            f"{state.spec.id}\t{state.status}\t"
            f"iter {state.iteration}/{state.spec.max_iterations}\t{state.phase}\t"
            f"{state.spec.package_path}\tupdated {state.updated_at}"
        )
        print(f"    brief:    {brief}")
        print(f"    gates:    {gates}")
        if residual_note:
            print(f"    residual: {residual_note}")


def print_queue_status(store: TicketStore, config: BuilderConfig) -> None:
    states = store.list()
    counts: dict[str, int] = {}
    for state in states:
        counts[state.status] = counts.get(state.status, 0) + 1
    lock_path = store.queue_root / "worker.lock"
    lock_state = "not held"
    if lock_path.is_file():
        try:
            held_by = int(lock_path.read_text(encoding="utf-8").strip())
        except ValueError:
            held_by = None
        if held_by is not None:
            lock_state = (
                f"held by pid {held_by} (running)"
                if _pid_is_alive(held_by)
                else f"stale (pid {held_by} is not running; next loop/run-ticket reclaims it)"
            )
    next_ticket = store.next_queued()
    print(f"root:  {config.root}")
    print(f"queue: {store.queue_root}")
    print(f"lock:  {lock_state}")
    print(f"total: {len(states)} ticket(s)")
    for status, count in sorted(counts.items()):
        print(f"  {status}: {count}")
    print(f"next queued: {next_ticket.spec.id if next_ticket else '(none)'}")


if __name__ == "__main__":
    raise SystemExit(main())
