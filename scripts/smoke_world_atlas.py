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

    smoke_alpha8(binary)
    print("PASS: typed decision chain, exact sum, absence, zero counts, retries, cold restore, "
          "checkpoint, retraction, history, grouped collect, portable decision evidence, and independent audit")


ALPHA8_ATLAS_PROGRAM = """
config Plan = AskBack(List<Str>) | Done

fn no_slots(): List<Str> = []

rel input pages: { id: Str, domain: Str, title: Str } key id
rel input questions: { parent: Str } key parent
rel input slots: { id: Str, parent: Str, slot: Str, ord: Int } key id

rel derived nonempty_slots = select {
  parent: s.parent,
  missing: collect(s.slot, s.ord, 16)
} from s in slots group by s.parent

rel derived empty_slots = select {
  parent: q.parent,
  missing: no_slots()
} from q in questions
where not exists s in slots where s.parent == q.parent

rel derived question_slots = union nonempty_slots, empty_slots key parent

decide plan for g in question_slots per parent {
  propose ask priority 1 when true = Plan::AskBack(g.missing)
}
""".strip()


def smoke_alpha8(binary: Path):
    with tempfile.TemporaryDirectory(prefix="brix_alpha8_smoke_") as scratch_str:
        scratch = Path(scratch_str)
        world = scratch / "world_v8"
        isolated_dir = scratch / "isolated_verifier"
        isolated_dir.mkdir(parents=True, exist_ok=True)
        prog_path = scratch / "atlas_v8.brix"
        prog_path.write_text(ALPHA8_ATLAS_PROGRAM, encoding="utf-8")

        def run(*args, cwd=scratch, expect_ok=True):
            command = [str(binary), "world", *map(str, args), "--json"]
            result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=120)
            data = json.loads(result.stdout)
            if expect_ok:
                require(result.returncode == 0 and data.get("ok") is True,
                        f"{command} failed ({result.returncode}): {result.stdout}\n{result.stderr}")
            else:
                require(result.returncode != 0 and data.get("ok") is False,
                        f"{command} unexpectedly succeeded: {data}")
            return data

        init = run("init", world, prog_path)
        manifest_digest = init["manifest_digest"]
        program_digest = init["program_digest"]

        seed_v8 = scratch / "seed_v8.json"
        seed_v8.write_text(json.dumps({
            "schema": "brix.world.batch@1",
            "expected_base_revision": 0,
            "idempotency_key": "atlas-v8-seed",
            "operations": [
                {"op": "upsert", "relation": "atlas_v8::pages", "key": "p1",
                 "tuple": {"id": "p1", "domain": "finance", "title": "Revenue Overview"}},
                {"op": "upsert", "relation": "atlas_v8::questions", "key": "q1",
                 "tuple": {"parent": "q1"}},
                {"op": "upsert", "relation": "atlas_v8::slots", "key": "s1",
                 "tuple": {"id": "s1", "parent": "q1", "slot": "volume", "ord": 1}},
                {"op": "upsert", "relation": "atlas_v8::slots", "key": "s2",
                 "tuple": {"id": "s2", "parent": "q1", "slot": "customer-grain", "ord": 2}},
            ]
        }), encoding="utf-8")

        rev1 = run("batch", world, seed_v8)
        rev1_digest = rev1["digest"]

        settled1 = run("decide", world)["settlements"]["atlas_v8::plan"]["q1"]
        require(settled1["candidate_name"] == "ask", f"Expected ask candidate at rev 1: {settled1}")
        require(settled1["value"]["variant"] == "AskBack", f"Expected AskBack variant: {settled1}")
        require([x["value"] for x in settled1["value"]["args"][0]["items"]] == ["volume", "customer-grain"],
                f"Unexpected collected slots at rev 1: {settled1}")

        ev_file = isolated_dir / "evidence_rev1.json"
        run("evidence", world, "q1", "--decide", "atlas_v8::plan", "--rev", 1,
            "--caller", "service:atlas", "--grant", "read:atlas", "--out", ev_file)

        # Fresh-process verification in isolated_dir without world path access
        ver1 = run("verify-evidence", ev_file,
                   "--expect-manifest", manifest_digest,
                   "--expect-program", program_digest,
                   "--expect-revision", 1,
                   "--expect-revision-digest", rev1_digest,
                   "--expect-decide", "atlas_v8::plan",
                   "--expect-key", "q1",
                   "--expect-caller", "service:atlas",
                   "--expect-grant", "read:atlas",
                   cwd=isolated_dir)
        require(ver1["receipt"]["authority"] == "runtime_check", f"Expected runtime_check receipt: {ver1}")

        # Typed revision-pinned surface query at rev 1
        pages1 = run("query", world, "atlas_v8::pages", "--rev", 1, "--expect-digest", rev1_digest, "--typed")
        require(pages1["entries"][0]["tuple"]["title"] == "Revenue Overview",
                f"Unexpected typed surface tuple: {pages1}")

        # Commit rev 2: retract all child slots -> parent-anchored empty list []
        retract_v8 = scratch / "retract_v8.json"
        retract_v8.write_text(json.dumps({
            "schema": "brix.world.batch@1",
            "expected_base_revision": 1,
            "idempotency_key": "atlas-v8-retract",
            "operations": [
                {"op": "remove", "relation": "atlas_v8::slots", "key": "s1"},
                {"op": "remove", "relation": "atlas_v8::slots", "key": "s2"},
            ]
        }), encoding="utf-8")
        run("batch", world, retract_v8)

        settled2 = run("decide", world)["settlements"]["atlas_v8::plan"]["q1"]
        require(settled2["candidate_name"] == "ask", f"Expected ask candidate at rev 2: {settled2}")
        require(settled2["value"]["variant"] == "AskBack", f"Expected AskBack variant: {settled2}")
        require(settled2["value"]["args"][0]["items"] == [],
                f"Expected empty list [] after final child retraction: {settled2}")

        # Historical evidence request at rev 1 after advancing to rev 2
        ev_hist_file = isolated_dir / "evidence_hist_rev1.json"
        run("evidence", world, "q1", "--decide", "atlas_v8::plan", "--rev", 1,
            "--caller", "service:atlas", "--grant", "read:atlas", "--out", ev_hist_file)
        ver_hist = run("verify-evidence", ev_hist_file,
                       "--expect-manifest", manifest_digest,
                       "--expect-program", program_digest,
                       "--expect-revision", 1,
                       "--expect-revision-digest", rev1_digest,
                       "--expect-decide", "atlas_v8::plan",
                       "--expect-key", "q1",
                       "--expect-caller", "service:atlas",
                       "--expect-grant", "read:atlas",
                       cwd=isolated_dir)
        require(ver_hist["receipt"]["authority"] == "runtime_check", f"Historical verify failed: {ver_hist}")

        # Stale compilation check: querying rev 2 with rev 1 digest must fail
        stale = run("query", world, "atlas_v8::pages", "--rev", 2, "--expect-digest", rev1_digest, expect_ok=False)
        require(stale.get("status") == "revision-digest-mismatch", f"Expected revision-digest-mismatch: {stale}")



if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("example", nargs="?", type=Path,
                        default=Path(__file__).resolve().parents[1] / "examples/world-atlas")
    args = parser.parse_args()
    smoke(args.binary, args.example)
