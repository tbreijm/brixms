# Typed decisions over changing evidence

This synthetic Atlas-style example exercises the alpha.6 world profile. An order
has zero or more line calculations. Exact grouped sums feed a typed quote, which
feeds review and dispatch decisions in the same revision. Separate availability
decisions retain parents without calculations, and an absence query lists them.

Run from the repository root (or an extracted release archive, using `./brix`):

```sh
cargo build -p brix-cli
BRIX=./target/debug/brix
$BRIX world init /tmp/atlas-world examples/world-atlas/atlas.brix
$BRIX world batch /tmp/atlas-world examples/world-atlas/seed.json
$BRIX world decide /tmp/atlas-world
$BRIX world query /tmp/atlas-world atlas::missing
$BRIX world explain /tmp/atlas-world o1 --decide atlas::dispatch
$BRIX world open-check /tmp/atlas-world --verify-state
$BRIX world batch /tmp/atlas-world examples/world-atlas/retract.json
$BRIX world decide /tmp/atlas-world
$BRIX world explain /tmp/atlas-world o1 --decide atlas::dispatch --rev 1
```

Use a fresh world directory. Initially `o1` ships with `Quoted(Quote { total: 12 })`
through all three stages; `o2` reports `MissingCalculation`. Removing both lines
withdraws `o1`'s quote, review and dispatch together. Both parent orders then
report `MissingCalculation`, while the historical revision retains the original
typed outcome. Missing evidence is represented by explicit domain policy; it
does not acquire a stronger verification grade.

Each CLI invocation opens the world in a new process. The automated walkthrough
also checks idempotent retries, persisted-state restore, settlement checkpoint
reads, upstream explanations and independent audit verification:

```sh
python3 scripts/smoke_world_atlas.py ./target/debug/brix
```

The verifier obtains its expected program from initialization and its expected
HEAD from a separate world read. In an application, obtain those expected pins
through your own trusted channel rather than accepting identifiers supplied only
by an untrusted audit bundle.
