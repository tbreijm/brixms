"""The `BrixClient` implementation.

Speaks the `brix.serve@1` JSON-lines protocol (ADR-0044,
``spec/adr/ADR-0044_Serve_Protocol.md``) to a `brix serve --stdio` child
process. Standard library only: `subprocess`, `json`, `threading`, `queue`,
`itertools`, `os`, `shutil`.
"""

from __future__ import annotations

import itertools
import json
import os
import queue
import shutil
import subprocess
import threading
import time
from typing import Any, Dict, List, Optional, Union

# A program or input source: either a filesystem path (resolved relative to
# the server process's own working directory) or inline text. Both forms
# mirror the wire shape from ADR-0044 section 4: `{"path": "..."}` or
# `{"source": "..."}`.
ProgramSpec = Union[str, os.PathLike, Dict[str, str]]


def path(p: Union[str, os.PathLike]) -> Dict[str, str]:
    """Build a by-path program/input spec: ``{"path": str(p)}``."""
    return {"path": str(p)}


def source(text: str) -> Dict[str, str]:
    """Build an inline program/input spec: ``{"source": text}``.

    `text` must be the *raw* source or `brix.input@N` envelope text, not a
    parsed structure — the server decodes it byte-for-byte through the same
    strict decoder a file would go through (see ADR-0044 section 4).
    """
    return {"source": text}


def _normalize_spec(spec: ProgramSpec) -> Dict[str, str]:
    if isinstance(spec, dict):
        return spec
    return path(spec)


def _normalize_inputs(
    inputs: Optional[List[ProgramSpec]],
) -> Optional[List[Dict[str, str]]]:
    if inputs is None:
        return None
    return [_normalize_spec(i) for i in inputs]



def _explain_params(candidate: str, entity: Optional[int]) -> Dict[str, Any]:
    """`entity` selects one instance of a per-entity `decide` block (ADR-0043)."""
    params: Dict[str, Any] = {"candidate": candidate}
    if entity is not None:
        params["entity"] = entity
    return params

class BrixError(Exception):
    """Base class for every error this client raises."""


class BrixProtocolError(BrixError):
    """The server returned a protocol-level error response (`ok: false`).

    This means the request itself was rejected — malformed JSON, an unknown
    method, or invalid params — *not* that the underlying Brix command
    failed (a rejected `check`/`run`/etc. is a normal, successful call that
    returns its failure inside the result dict; see `BrixClient.call`).
    """

    def __init__(self, code: str, message: str):
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class BrixTimeoutError(BrixError):
    """No response was received for a request within its timeout."""


class BrixProcessError(BrixError):
    """The `brix serve --stdio` process exited or its pipes closed unexpectedly."""


def _default_brix_bin() -> str:
    """Resolve the `brix` executable: `BRIX_BIN` env var, else `brix` on `PATH`."""
    env_bin = os.environ.get("BRIX_BIN")
    if env_bin:
        return env_bin
    found = shutil.which("brix")
    if found:
        return found
    return "brix"


class BrixClient:
    """A client for one `brix serve --stdio` subprocess.

    Use as a context manager to guarantee the child process is cleaned up::

        with BrixClient() as client:
            result = client.run(program=path("examples/shipping.brix"))

    Or manage the lifecycle explicitly with `close()`.

    Every typed method (`check`, `run`, `why`, `whynot`, `audit`, `verify`,
    `test`, `kb_*`) returns the command's own JSON result dict (exactly what
    `--json` would print for the equivalent CLI invocation) and raises
    `BrixProtocolError` only for a *protocol*-level failure. A rejected or
    Unknown command outcome is not an exception: check `result["ok"]` /
    `result["status"]`, matching how `--json` already represents it.
    """

    def __init__(
        self,
        brix_bin: Optional[str] = None,
        cwd: Optional[Union[str, os.PathLike]] = None,
        timeout: Optional[float] = 30.0,
        extra_args: Optional[List[str]] = None,
    ):
        """Spawn `brix serve --stdio`.

        Args:
            brix_bin: path to the `brix` executable. Defaults to the
                `BRIX_BIN` environment variable, then `brix` on `PATH`.
            cwd: working directory for the child process (program/input
                paths given as `path(...)` are resolved relative to this).
            timeout: default timeout, in seconds, for a call that does not
                override it. `None` means wait indefinitely.
            extra_args: extra arguments appended after `serve --stdio`
                (reserved for future use; unused by the protocol today).
        """
        self._bin = brix_bin or _default_brix_bin()
        self._default_timeout = timeout
        self._id_counter = itertools.count(1)
        self._closed = False
        self._lock = threading.Lock()
        # Ids of requests that timed out. The server answers strictly in
        # order, so each one's response still arrives, ahead of any later
        # request's; it is discarded when read.
        self._abandoned: set = set()

        args = [self._bin, "serve", "--stdio"] + (extra_args or [])
        try:
            self._proc = subprocess.Popen(
                args,
                cwd=str(cwd) if cwd is not None else None,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                bufsize=1,  # line-buffered
            )
        except OSError as e:
            raise BrixProcessError(f"failed to start '{self._bin} serve --stdio': {e}") from e

        self._out_queue: "queue.Queue[Optional[str]]" = queue.Queue()
        self._stderr_lines: List[str] = []
        self._out_thread = threading.Thread(
            target=self._pump_stdout, name="brix-client-stdout", daemon=True
        )
        self._err_thread = threading.Thread(
            target=self._pump_stderr, name="brix-client-stderr", daemon=True
        )
        self._out_thread.start()
        self._err_thread.start()

    # -- process plumbing ----------------------------------------------

    def _pump_stdout(self) -> None:
        assert self._proc.stdout is not None
        try:
            for line in self._proc.stdout:
                self._out_queue.put(line)
        finally:
            self._out_queue.put(None)  # EOF sentinel

    def _pump_stderr(self) -> None:
        assert self._proc.stderr is not None
        for line in self._proc.stderr:
            self._stderr_lines.append(line.rstrip("\n"))

    def _next_id(self) -> int:
        return next(self._id_counter)

    # -- lifecycle -------------------------------------------------------

    def __enter__(self) -> "BrixClient":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def close(self, timeout: float = 5.0) -> None:
        """Signal EOF (close stdin) and wait for clean shutdown, then ensure
        the process is gone. Idempotent."""
        if self._closed:
            return
        self._closed = True
        try:
            if self._proc.stdin is not None:
                self._proc.stdin.close()
        except OSError:
            pass
        try:
            self._proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            try:
                self._proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                pass

    @property
    def closed(self) -> bool:
        return self._closed or self._proc.poll() is not None

    # -- low-level protocol call -----------------------------------------

    def call(
        self,
        method: str,
        params: Optional[Dict[str, Any]] = None,
        request_id: Optional[Any] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Send one request and return its full response envelope.

        Returns the raw `{"schema", "id", "ok", "exit_code"?, "result"?,
        "error"?}` object. Raises `BrixProtocolError` if `ok` is false,
        `BrixTimeoutError` if no response arrives in time, and
        `BrixProcessError` if the server's stdout/process ends unexpectedly.
        Prefer the typed methods below unless you need this envelope's
        `exit_code` or are calling a method this client does not wrap yet.
        """
        if self.closed:
            raise BrixProcessError("brix serve --stdio process is not running")
        if request_id is None:
            request_id = self._next_id()
        request = {"id": request_id, "method": method, "params": params or {}}
        line = json.dumps(request)

        with self._lock:
            try:
                assert self._proc.stdin is not None
                self._proc.stdin.write(line + "\n")
                self._proc.stdin.flush()
            except (BrokenPipeError, OSError) as e:
                raise BrixProcessError(f"failed to write request: {e}") from e

            effective_timeout = timeout if timeout is not None else self._default_timeout
            deadline = time.monotonic() + effective_timeout
            while True:
                remaining = deadline - time.monotonic()
                try:
                    raw = self._out_queue.get(timeout=max(remaining, 0))
                except queue.Empty:
                    self._abandoned.add(request_id)
                    raise BrixTimeoutError(
                        f"no response for method '{method}' within {effective_timeout}s"
                    ) from None
                if raw is None:
                    stderr_tail = "\n".join(self._stderr_lines[-20:])
                    raise BrixProcessError(
                        "brix serve --stdio closed stdout unexpectedly"
                        + (f"; stderr:\n{stderr_tail}" if stderr_tail else "")
                    )
                try:
                    response = json.loads(raw)
                except json.JSONDecodeError as e:
                    raise BrixProcessError(
                        f"server sent a non-JSON response line: {e}: {raw!r}"
                    ) from e
                late_id = response.get("id")
                if late_id != request_id and late_id in self._abandoned:
                    # The late answer to an earlier, timed-out request.
                    self._abandoned.discard(late_id)
                    continue
                break

        if response.get("id") != request_id:
            raise BrixProtocolError(
                "id-mismatch",
                f"expected response id {request_id!r}, got {response.get('id')!r} "
                "(responses were read out of order, or the session is being shared "
                "across threads without external synchronization)",
            )

        if not response.get("ok", False):
            error = response.get("error") or {}
            raise BrixProtocolError(
                error.get("code", "unknown-error"),
                error.get("message", "no message"),
            )

        return response

    def result(
        self,
        method: str,
        params: Optional[Dict[str, Any]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Call `method` and return just its `result` dict."""
        return self.call(method, params, timeout=timeout)["result"]

    # -- hello -------------------------------------------------------------

    def hello(self, timeout: Optional[float] = None) -> Dict[str, Any]:
        """Return the server's `brix.serve.hello@1` object (versions, methods)."""
        return self.result("hello", {}, timeout=timeout)

    # -- program-bearing methods -------------------------------------------

    def _program_call(
        self,
        method: str,
        program: ProgramSpec,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        extra: Optional[Dict[str, Any]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        params: Dict[str, Any] = {"program": _normalize_spec(program)}
        normalized_inputs = _normalize_inputs(inputs)
        if normalized_inputs is not None:
            params["inputs"] = normalized_inputs
        if package_paths is not None:
            params["package_paths"] = list(package_paths)
        if extra:
            params.update(extra)
        return self.result(method, params, timeout=timeout)

    def check(
        self,
        program: ProgramSpec,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix check` — parse, resolve imports, lower, and preflight check."""
        return self._program_call("check", program, inputs, package_paths, timeout=timeout)

    def run(
        self,
        program: ProgramSpec,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix run` — execute a finite-decision deliberation plan to completion."""
        return self._program_call("run", program, inputs, package_paths, timeout=timeout)

    def why(
        self,
        program: ProgramSpec,
        candidate: str,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
        entity: Optional[int] = None,
    ) -> Dict[str, Any]:
        """`brix why` — explain why `candidate` was admitted or selected."""
        return self._program_call(
            "why", program, inputs, package_paths, extra=_explain_params(candidate, entity), timeout=timeout
        )

    def whynot(
        self,
        program: ProgramSpec,
        candidate: str,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
        entity: Optional[int] = None,
    ) -> Dict[str, Any]:
        """`brix whynot` — explain why `candidate` was not admitted or not selected."""
        return self._program_call(
            "whynot",
            program,
            inputs,
            package_paths,
            extra=_explain_params(candidate, entity),
            timeout=timeout,
        )

    def audit(
        self,
        program: ProgramSpec,
        bundle_out: Union[str, os.PathLike],
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        force: bool = False,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix audit` — run and audit a plan, writing a bundle to `bundle_out`
        on the *server's* filesystem (a local path if the server is local)."""
        return self._program_call(
            "audit",
            program,
            inputs,
            package_paths,
            extra={"bundle_out": str(bundle_out), "force": force},
            timeout=timeout,
        )

    def verify(
        self,
        program: ProgramSpec,
        bundle: Union[str, os.PathLike],
        expect_program: str,
        profile: str = "finite-decision",
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix verify` — verify an audit bundle against source and the
        externally pinned `expect_program` hex id. `profile` is
        `"finite-decision"` (default) or `"l3-v1"`."""
        return self._program_call(
            "verify",
            program,
            inputs,
            package_paths,
            extra={
                "bundle": str(bundle),
                "expect_program": expect_program,
                "profile": profile,
            },
            timeout=timeout,
        )

    def test(
        self,
        files: List[Union[str, os.PathLike]],
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix test` — run one or more `brix.test@1` regression suite files."""
        return self.result("test", {"files": [str(f) for f in files]}, timeout=timeout)

    # -- kb ------------------------------------------------------------

    def _kb_call(
        self,
        op: str,
        kb_dir: Union[str, os.PathLike],
        package_paths: Optional[List[str]] = None,
        extra: Optional[Dict[str, Any]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        params: Dict[str, Any] = {"dir": str(kb_dir)}
        if package_paths is not None:
            params["package_paths"] = list(package_paths)
        if extra:
            params.update(extra)
        return self.result(f"kb.{op}", params, timeout=timeout)

    def kb_init(
        self,
        kb_dir: Union[str, os.PathLike],
        program: ProgramSpec,
        inputs: Optional[List[ProgramSpec]] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb init` — create a knowledge base and its first revision."""
        extra: Dict[str, Any] = {"program": _normalize_spec(program)}
        normalized_inputs = _normalize_inputs(inputs)
        if normalized_inputs is not None:
            extra["inputs"] = normalized_inputs
        return self._kb_call("init", kb_dir, package_paths, extra, timeout=timeout)

    def kb_assert(
        self,
        kb_dir: Union[str, os.PathLike],
        inputs: List[ProgramSpec],
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb assert` — upsert named input values, creating a new revision."""
        return self._kb_call(
            "assert",
            kb_dir,
            package_paths,
            {"inputs": _normalize_inputs(inputs)},
            timeout=timeout,
        )

    def kb_retract(
        self,
        kb_dir: Union[str, os.PathLike],
        names: List[str],
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb retract` — remove named input values, creating a new revision."""
        return self._kb_call(
            "retract", kb_dir, package_paths, {"names": list(names)}, timeout=timeout
        )

    def kb_program(
        self,
        kb_dir: Union[str, os.PathLike],
        program: ProgramSpec,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb program` — change the knowledge base's program."""
        return self._kb_call(
            "program",
            kb_dir,
            package_paths,
            {"program": _normalize_spec(program)},
            timeout=timeout,
        )

    def kb_log(
        self,
        kb_dir: Union[str, os.PathLike],
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb log` — list every revision, oldest first."""
        return self._kb_call("log", kb_dir, package_paths, timeout=timeout)

    def kb_show(
        self,
        kb_dir: Union[str, os.PathLike],
        rev: Optional[int] = None,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb show` — show one revision's full decision report
        (defaults to the current HEAD when `rev` is omitted)."""
        extra = {"rev": rev} if rev is not None else None
        return self._kb_call("show", kb_dir, package_paths, extra, timeout=timeout)

    def kb_diff(
        self,
        kb_dir: Union[str, os.PathLike],
        rev_a: int,
        rev_b: int,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb diff` — show what changed between two revisions, and why."""
        return self._kb_call(
            "diff", kb_dir, package_paths, {"rev_a": rev_a, "rev_b": rev_b}, timeout=timeout
        )

    def kb_audit(
        self,
        kb_dir: Union[str, os.PathLike],
        rev: int,
        bundle_out: Union[str, os.PathLike],
        force: bool = False,
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb audit` — emit a standard audit bundle for one revision."""
        return self._kb_call(
            "audit",
            kb_dir,
            package_paths,
            {"rev": rev, "bundle_out": str(bundle_out), "force": force},
            timeout=timeout,
        )

    def kb_verify(
        self,
        kb_dir: Union[str, os.PathLike],
        package_paths: Optional[List[str]] = None,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """`brix kb verify` — verify the whole knowledge base end to end."""
        return self._kb_call("verify", kb_dir, package_paths, timeout=timeout)
