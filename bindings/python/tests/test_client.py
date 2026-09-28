"""Tests for BrixClient against the built `brix serve --stdio` binary, driven
over examples/ from the repository root (ADR-0044)."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brix import BrixClient, BrixProtocolError, BrixTimeoutError, path, source  # noqa: E402


@pytest.fixture
def client(brix_bin, examples_dir):
    with BrixClient(brix_bin=brix_bin, cwd=examples_dir) as c:
        yield c


def test_hello(client):
    hello = client.hello()
    assert hello["schema"] == "brix.serve.hello@1"
    assert hello["protocol"] == "brix.serve@1"
    assert "check" in hello["methods"]
    assert "run" in hello["methods"]
    assert "kb.init" in hello["methods"]
    assert isinstance(hello["toolchain_version"], str)


def test_check_by_path(client):
    result = client.check(program=path("examples/shipping.brix"))
    assert result["schema"] == "brix.cli.result@1"
    assert result["command"] == "check"
    assert result["ok"] is True
    assert result["status"] == "accepted"


def test_run_by_path_selects_ship(client):
    result = client.run(program=path("examples/shipping.brix"))
    assert result["ok"] is True
    assert result["status"] == "selected"
    assert result["decision"]["candidate"] == "ship"


def test_run_with_inline_source(client, examples_dir):
    text = (examples_dir / "examples" / "shipping.brix").read_text()
    result = client.run(program=source(text))
    assert result["ok"] is True
    assert result["decision"]["candidate"] == "ship"


def test_why_and_whynot(client):
    why = client.why(program=path("examples/shipping.brix"), candidate="ship")
    assert why["ok"] is True
    assert why["explanation"]["candidate"] == "ship"

    whynot = client.whynot(program=path("examples/shipping.brix"), candidate="expedite")
    assert whynot["ok"] is True
    assert whynot["command"] == "whynot"


def test_inline_vs_file_input_identical_snapshot(client, examples_dir):
    input_text = (examples_dir / "examples" / "shipping-input.json").read_text()

    by_file = client.run(
        program=path("examples/shipping-input.brix"),
        inputs=[path("examples/shipping-input.json")],
    )
    by_source = client.run(
        program=path("examples/shipping-input.brix"),
        inputs=[source(input_text)],
    )
    assert by_file["ok"] is True
    assert by_source["ok"] is True
    assert by_file["input_snapshot"] == by_source["input_snapshot"]
    assert by_file["program"] == by_source["program"]


def test_inline_duplicate_key_rejected(client):
    dup = (
        '{"schema":"brix.input@1","values":{'
        '"stock":{"type":"int","value":"1"},'
        '"stock":{"type":"int","value":"2"}}}'
    )
    result = client.run(
        program=path("examples/shipping-input.brix"),
        inputs=[source(dup)],
    )
    assert result["ok"] is False
    assert result["status"] == "rejected"
    assert any("duplicate" in d for d in result["diagnostics"])


def test_unknown_program_reports_rejected_not_exception(client):
    # A missing declared input is a normal (non-exceptional) rejected result.
    result = client.run(program=path("examples/shipping-input.brix"))
    assert result["ok"] is False
    assert result["status"] == "rejected"


def test_audit_then_verify_round_trip(client, tmp_path):
    bundle_path = tmp_path / "bundle.bin"
    audit = client.audit(
        program=path("examples/shipping.brix"),
        bundle_out=str(bundle_path),
    )
    assert audit["ok"] is True
    assert bundle_path.exists()

    verify = client.verify(
        program=path("examples/shipping.brix"),
        bundle=str(bundle_path),
        expect_program=audit["program"],
    )
    assert verify["ok"] is True
    assert verify["command"] == "verify"


def test_test_method(client):
    result = client.test(files=["examples/shipping.test.json"])
    assert result["schema"] == "brix.test.result@1"


def test_kb_session(client, tmp_path):
    kb_dir = tmp_path / "kb"
    init = client.kb_init(
        kb_dir=str(kb_dir),
        program=path("examples/shipping-input.brix"),
        inputs=[path("examples/shipping-input.json")],
    )
    assert init["ok"] is True

    show = client.kb_show(kb_dir=str(kb_dir))
    assert show["ok"] is True

    log = client.kb_log(kb_dir=str(kb_dir))
    assert log["ok"] is True


def test_protocol_error_on_bad_params(client):
    with pytest.raises(BrixProtocolError):
        client.call("check", {"program": {}})  # neither path nor source


def test_protocol_error_on_unknown_method(client):
    with pytest.raises(BrixProtocolError):
        client.call("no-such-method", {})


def test_context_manager_cleans_up_process(brix_bin, examples_dir):
    c = BrixClient(brix_bin=brix_bin, cwd=examples_dir)
    with c:
        c.hello()
    assert c.closed


def test_timeout_raises_on_a_process_that_never_responds(brix_bin, examples_dir):
    # A method the server does not recognize as a request at all: feed raw
    # bytes that are not even a newline-terminated JSON object, to exercise
    # the client's own timeout path rather than the server's error path.
    # We simulate "no response" by asking with an unreasonably small timeout
    # against a real, slower-than-that call is impractical to construct
    # deterministically, so instead we verify the timeout plumbing directly:
    # a call with timeout=0 must not hang forever.
    with BrixClient(brix_bin=brix_bin, cwd=examples_dir, timeout=30.0) as c:
        with pytest.raises(BrixTimeoutError):
            c.call("hello", {}, timeout=0.0)


def test_pipelined_low_level_calls_share_one_process(brix_bin, examples_dir):
    # Not true pipelining (this client is synchronous per call), but proves
    # multiple sequential calls against the same session work and share
    # server-side sequential processing without desync.
    with BrixClient(brix_bin=brix_bin, cwd=examples_dir) as c:
        results = [
            c.run(program=path("examples/shipping.brix"))["decision"]["candidate"]
            for _ in range(5)
        ]
        assert results == ["ship"] * 5


SLOW_PROGRAM = """config D = Go
rule n() = count([{xs}], a => any([{xs}], b => a + b < 0))
propose go priority 1 when n == 0 = Go
commit d from (go)
"""


def test_timeout_does_not_desynchronize_the_session(client):
    """A request that times out still gets its answer later; the client
    discards it, so the next call receives its own response."""
    xs = ", ".join(str(i) for i in range(256))
    slow = source(SLOW_PROGRAM.format(xs=xs))
    with pytest.raises(BrixTimeoutError):
        client.run(program=slow, timeout=0.001)

    result = client.check(program=path("examples/shipping.brix"), timeout=60)
    assert result["command"] == "check"
    assert result["ok"] is True
    # And the session keeps working after that.
    assert client.run(program=path("examples/shipping.brix"))["decision"]["candidate"] == "ship"
