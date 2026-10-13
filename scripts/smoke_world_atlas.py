#!/usr/bin/env python3
"""Exercise the alpha.6 world workflow through fresh CLI processes.

Usage: python3 scripts/smoke_world_atlas.py /path/to/brix [examples/world-atlas]
The example path may point into an extracted release package.
"""

import argparse
import json
from pathlib import Path
import subprocess
import tempfile


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def smoke(binary, example):
    binary, example = Path(binary).resolve(), Path(example).resolve()
    with tempfile.TemporaryDirectory(prefix="brix_alpha6_smoke_") as scratch:
        scratch = Path(scratch)
        world = scratch / "world"

        def run(*args):
            command = [str(binary), "world", *map(str, args), "--json"]
            result = subprocess.run(command, cwd=scratch, capture_output=True,
                                    text=True, timeout=120)
            require(result.returncode == 0,
                    f"{command} failed ({result.returncode}): {result.stdout}\n{result.stderr}")
            data = json.loads(result.stdout)
            require(data.get("ok") is True, f"{command}: {data}")
            return data

        initialized = run("init", world, example / "atlas.brix")
        program = initialized["program_digest"]
        first = run("batch", world, example / "seed.json")
        require(first["revision"] == 1, f"Seed revision: {first}")
        replay = run("batch", world, example / "seed.json")
        require(replay["is_idempotent_replay"] is True,
                f"Retry did not return original commit: {replay}")
        require(replay["digest"] == first["digest"],
                "Retry changed revision identity")

        opened = run("open-check", world, "--verify-state")
        require(opened["open_mode"] == "restored", f"Unexpected replay: {opened}")
        settled = run("decide", world)["settlements"]
        quote = {
            "type": "sum", "nominal": "atlas::QuoteResult", "variant": "Quoted",
            "args": [{"type": "record", "nominal": "atlas::Quote",
                      "fields": [{"name": "total", "value": {"type": "int", "value": "12"}}]}],
        }
        for stage, candidate in (("quote", "quoted"), ("review", "approved"),
                                 ("dispatch", "ship")):
            row = settled[f"atlas::{stage}"]["o1"]
            require(row["candidate_name"] == candidate, f"Wrong {stage}: {row}")
            require(row["value"] == quote, f"Typed quote lost at {stage}: {row}")
        require(settled["atlas::availability"]["o2"]["value"]["variant"] == "MissingCalculation",
                "Empty parent has no missing-calculation result")
        missing = run("query", world, "atlas::missing")
        require([e["tuple"]["id"] for e in missing["entries"]] == ["o2"],
                f"Absence query incorrect: {missing}")
        explanation = run("explain", world, "o1", "--decide", "atlas::dispatch")["explanation"]
        require({x["decide"] for x in explanation["upstream_settlements"]}
                == {"atlas::quote", "atlas::review"},
                f"Incomplete decision-chain explanation: {explanation}")

        # Reading a checkpoint must preserve typed payloads in a fresh process.
        run("open-check", world, "--refresh-state")
        require(run("decide", world)["settlements"] == settled,
                "Checkpoint changed typed decisions")

        run("batch", world, example / "retract.json")
        require(run("open-check", world, "--verify-state")["open_mode"] == "restored",
                "Retraction state was not restorable")
        after = run("decide", world)["settlements"]
        for stage in ("quote", "review", "dispatch"):
            require(not after.get(f"atlas::{stage}"), f"Stale {stage} after retraction: {after}")
        for entity in ("o1", "o2"):
            require(after["atlas::availability"][entity]["value"]["variant"] == "MissingCalculation",
                    f"Missing zero-count result for {entity}: {after}")
        missing = run("query", world, "atlas::missing")
        require({e["tuple"]["id"] for e in missing["entries"]} == {"o1", "o2"},
                f"Absence did not return after last support removal: {missing}")
        historical = run("explain", world, "o1", "--decide", "atlas::dispatch", "--rev", 1)
        require(historical["explanation"]["value"] == quote,
                f"Historical typed outcome changed: {historical}")

        # Audit uses a separately read HEAD pin and init's program pin.
        head = run("show", world)["revision"]["revision_digest"]
        bundle = scratch / "world.brixworld"
        run("audit", world, "--out", bundle)
        verified = run("verify", "--expect-head", head, "--expect-program", program, bundle)
        require(verified.get("status") == "audited", f"Independent audit failed: {verified}")
        print("PASS: typed decision chain, exact sum, absence, zero counts, retries, cold restore, "
              "checkpoint, retraction, history, and independent audit")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("example", nargs="?", type=Path,
                        default=Path(__file__).resolve().parents[1] / "examples/world-atlas")
    args = parser.parse_args()
    smoke(args.binary, args.example)
