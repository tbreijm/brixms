# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

**Software is good at producing an answer. It is much worse at preserving why
that answer was allowed to become the answer. BrixMS is a language and runtime
for that second problem.**

## The problem

Imagine a system deciding whether an order may ship. It combines a stock
measurement, an estimated delivery time, a pricing rule, a compliance rule, and
the policy in force today. Several outcomes may be possible. Some inputs were
measured, some derived, and some formally proved. Tomorrow one of them may be
corrected.

A conventional program can certainly calculate `ship = true`. But after it
stores that boolean, the important distinctions are easy to lose:

- What other outcomes were possible?
- Which policy selected this one?
- Which facts were measured, inferred, replay-checked, or proved?
- Can another process reproduce the decision from the same inputs?
- If one input changes, what must be reconsidered—and what can safely remain?

Logs help, but they usually record what code ran, not why the resulting claim
deserved a particular level of trust. A database can retain history, but history
alone does not say which authority was entitled to publish a conclusion. A rule
engine can derive many facts, but deriving possibilities and committing one
operational reality are different acts.

Systems faced with incomplete, contested, or revisable information therefore
tend toward one of two failures: they pretend there is a single truth and
silently overwrite what came before, or they retain every possibility without a
disciplined way to commit and act. Neither result is easy to audit later.

## Why the existing pieces are not enough by themselves

BrixMS builds on established tools and theories; it does not claim they are
defective. They solve different parts of the problem:

| Existing approach | What it is good at | What remains separate |
| --- | --- | --- |
| Databases and rule engines | Storing facts and deriving consequences | The evidence grade and authority behind a committed result |
| Workflow and event systems | Choosing and recording actions | Replayable semantic justification rather than an after-the-fact log |
| Proof assistants | Checking a theorem in an explicit context | Running a changing world under policy and bounded resources |
| Incremental dataflow | Updating results from small changes efficiently | Choosing among admissible alternatives and grading the result |

One application can combine all four, but their boundaries then live in glue
code: a convention says which log counts as an audit, which boolean came from a
proof, which cache is valid under a new revision, and which component may claim
success. Those conventions are exactly where evidence can be dropped or
silently upgraded.

## Why a whole language?

The first versions of Brix were libraries and runtimes. That proved the
execution idea, but it also exposed the limit of treating the important rules
as API discipline. If evidence, policy, identity, and provenance are ordinary
optional values, ordinary code can forget them.

Brix makes them part of the program instead:

- `config` and `witness` are the primitive semantic objects; `rule`, source
  `regime` syntax, and runtime providers construct or present witness
  relations without becoming additional semantic entities;
- evidence grades such as `@Derived`, `@Audited`, and `@Proven` are checked, not
  comments;
- composition preserves the witness explaining a result;
- context, policy, and history are bound into canonical identities;
- illegal evidence upgrades are rejected at publication boundaries.

The point of a new language is not novel punctuation. It is to make the
accountability rules structural from source text through execution, audit, and
proof—so that bypassing them is not the easiest programming path.

**Brix** is that language. **SOC (Settlement-Oriented Computing)** is the
paradigm underneath it: settlement is the organizing idea in the same way that
objects organize object-oriented systems.

> BrixMS is experimental and pre-release. The core runtime, proof kernel,
> incremental engine, and first end-to-end language workflows exist today. The
> language, verification transport, and broader execution profiles are still
> being completed.

## What is BrixMS for?

BrixMS is aimed at programs that must act while information is still partial,
disputed, or subject to revision—and must later explain the context, policy,
history, and evidence behind the action.

That makes the architecture relevant to rule engines, simulations, planning
systems, policy-driven automation, and other stateful systems where:

- several next actions may be valid, but commitment must be deterministic;
- results must be replayable or independently checked;
- “not established” must remain different from “false”;
- changing a small part of a large world should trigger local work, not a full
  recomputation;
- users need to inspect not only a result, but its derivation and proof status.

It is not presented as a production-ready solution for those domains yet. They
describe the class of problems the design is being built to handle.

## Where it came from

BrixMS is a continuation of public work, not a new name placed on a blank
history.

### 2019: the Brix formalism

The documented line begins with Tony Reijm's 2019 TU Delft EPA thesis. It
described models assembled from context-independent “brix” and a coarse
detect–execute cycle: find combinations that satisfy an interaction, turn them
into events, execute them, and repeat. The current
[`article outline`](./docs/BrixMS_Scientific_Article_Outline.md) records the
source sections and, importantly, the thesis's own admission that the Cartesian
detection phase becomes intractable. That limitation is the historical reason
the current project treats cost proportional to the whole world as a semantic
failure, not a performance task to postpone.

### The first open-source implementation

The original browser implementation remains public at
[`tbreijm/tbreijm.github.io`](https://github.com/tbreijm/tbreijm.github.io). Its
model was already recognizable: structures supplied properties; roles selected
context-specific views and access; behaviours plus constraints formed
interactions; collision detection dynamically bound structures to roles and
generated events until the event set was exhausted. The repository includes the
JavaScript implementation and a
[runnable browser demo](https://tbreijm.github.io/) under the MIT license.

That is the project’s detect–execute lineage. It established the useful idea:
do not hard-wire every object-to-object call; detect which pieces fit the roles
of an interaction, then execute the resulting event.

### From collisions to indexed relations

A later TypeScript/hypergraph iteration made those relationships explicit and
moved detection toward incidence indices. The archived
[`Ring 0 build plan`](./spec/archive/Ring0_Build_Plan.md) records the connection
directly: the global incidence index was the production form of the original
collision-loop primitive. It also records the reference-oracle discipline that
survives today—keep a deliberately simple implementation and differentially
test the optimized engine against it.

### From detect–execute to settlement

The current Rust implementation is a ground-up SOC-native rebuild. It preserves
the original concern—discover applicable interactions without scanning an
inert world—but broadens the question from *what can fire?* to *what may this
system commit, under this policy, with what evidence?*

The evolution is visible in the architecture:

```text
original Brix       detect matching structures -> generate and execute events
hypergraph engine   index incidence -> update affected relations
current BrixMS      propose candidates -> settle one -> audit or prove it
```

This is conceptual and experimental continuity, not source compatibility. The
legacy engine was retained as a differential oracle during the SOC transition
and then deleted after the native implementation reached parity.

### Prior art, stated openly

The broader design also stands on known work: Datalog and Datomic for facts and
rules, dependent type systems and Lean for proofs as values, incremental and
differential dataflow for O(Δ) maintenance, provenance research for derivation
tracking, and category/coalgebra work for relational composition and
saturation. [`ADR-0010`](./spec/adr/ADR-0010_SOC_Language_Design.md) summarizes
that lineage; the article outline records the later prior-art review that
explicitly rejects claiming the categorical machinery as novel.

The intended contribution is the synthesis and its enforced accountability
discipline: a committed step factors through declared logged generators;
replay, settlement, and proof have separate authorities; and evidence cannot be
upgraded merely because one component says so.

## How BrixMS answers the problem

Four ideas carry the distinction between possibility, commitment, and evidence
all the way through the system.

### 1. Configurations and witnesses

A configuration is a state, value, model, program fragment, or world fragment.
A witness records a meaningful relationship or transition between two
configurations. Its `RegimeId` is provenance for the interpretation under
which that witness is meaningful; it is not a discovery-provider identity.

Mathematically, configurations are the objects of one category and witnesses
are its arrows. In everyday use, that means every important transition has a
nameable, content-addressed explanation.

### 2. Deliberation is plural; commitment is singular

More than one non-ontological witness provider may present the same or distinct
candidate witnesses. Candidates are deduplicated by witness and successor; an
admissibility policy filters them, and a keyed calendar selects exactly one in
a stable order. The committed journal therefore does not depend on thread
timing, map iteration, or provider identity.

```text
world + policy + history
          |
          v
 witness providers present possibilities
          |
          v
   admissibility filter
          |
          v
 deterministic calendar -----> committed step (@Derived)
                                      |
                                      +---- replay audit (@Audited)
                                      |
                                      +---- proof kernel (@Proven)
```

### 3. Evidence has grades

BrixMS does not collapse every outcome into `true` or `false`:

```text
       Proven       Refuted      kernel-certified, incomparable poles
           \         /
             Audited             replay verified
                |
             Derived             committed within a revision
                |
             Measured            certified external result
                |
             Unknown             no truth commitment
```

Each grade has one authority. The settlement kernel may publish `Derived`; the
audit checker may publish `Audited`; only a proof kernel may publish `Proven` or
`Refuted`. Resource exhaustion, unsupported input, incomplete search, and
failed replay remain `Unknown`.

Strengthening a result creates a new judgement linked to the earlier evidence.
It never edits the old claim or silently rounds it upward.

### 4. Cost follows change

The central performance rule is O(Δ): work per committed step must scale with
the changed configurations and their index fanout, not with all inert state in
the world.

```text
cost(step) ∝ |Δ| × fanout
doubling inert |world| ⇒ no per-step cost increase
```

The repository keeps both implementations needed to enforce this honestly: a
simple recompute-the-world oracle and the real incremental engine. The active
[`o_delta_gate`](./crates/soc-core/tests/o_delta_gate.rs) proves that the naïve
path grows with world size while the incremental path stays flat.

## What works now

The project has moved beyond a paper design. These paths are implemented and
covered by executable gates:

| Layer | Current implementation |
| --- | --- |
| Brix language | Hand-written lexer/parser; functions and bindings; records and algebraic sums; directly recursive and parameterized configurations; matching; arithmetic and comparison; grade annotations |
| Type realization | Tree-shaped derivations, conflict reporting, declared function contracts, certified match coverage, and honest per-result grade caps |
| Command line | `check`, `run`, `audit`, `verify`, `why`, and `whynot` |
| Settlement runtime | Admission policies, deterministic keyed selection, transactional candidate deltas, persistent state, append-only journals, and deterministic replay |
| Incremental engine | Materialized candidate views, footprint indexing, differential agreement with the naïve oracle, and the green O(Δ) gate |
| Audit | Replay-verified decompositions, authority-checked `Audited` publication, oracle-bound receipts, and source-re-derived L3 manifests |
| Saturation | Administrative versus realizing steps, certified quiescence, bounded divergence evidence, weak bisimulation/refinement, and closure checking |
| Proof | A small dependent kernel with explicit proof terms, composition and tensor rules, primitive relations, canonical certificate envelopes, and adversarial vectors |
| Reproducibility | Pinned Rust toolchain, canonical encodings, frozen vectors, independent cross-checks, deterministic-order lints, and artifact-drift CI |

Execution profiles currently in the workspace:

- `brix check` exercises native type-realization over top-level bindings (`soc-regimes`)
  or runs preflight verification on finite-decision modules;
- `brix.l3.finite-decision@1` ([ADR-0030](./spec/adr/ADR-0030_Finite_Decision_Alpha.md),
  [ADR-0031](./spec/adr/ADR-0031_External_Input_Alpha.md))
  implements the finite-decision alpha deliberation profile across `crates/soc-regimes` and `crates/brix-lower`,
  evaluating complete candidate frontiers, structured rejection reasons,
  external operational inputs with strict schema validation and bounded disjoint shards (alpha.3), and
  deterministic calendar selection at phase zero. Deliberated outcomes are committed as `@Derived`
  at runtime; the runtime decision remains `@Derived`, while successful independent replay issues and
  verifies separate `@Audited` audit receipts via audit bundle verification;
- `brix run`, `audit`, `verify`, `why`, and `whynot` drive the finite-decision alpha workflow (with repeatable `--input` support);
- `crates/brix-lower` additionally contains Stages A–C of L3 v2 derivation
  ([ADR-0027](./spec/adr/ADR-0027_L3_V2_Derivation.md)).

The parser recognizes some designed syntax that downstream execution profiles do not
yet implement. Those constructs are refused before execution; they do not
become guessed results.

## Try it

Prebuilt release archives are published for **`aarch64-apple-darwin`** (macOS Apple Silicon) and **`x86_64-unknown-linux-gnu`** (Linux x86_64) on GitHub Releases.

### Prebuilt archive installation

#### macOS (Apple Silicon: `aarch64-apple-darwin`)

1. Download the release archive and SHA256 checksum file:
   ```bash
   curl -LO https://github.com/tbreijm/brixms/releases/download/v0.1.0-alpha.3/brix-v0.1.0-alpha.3-aarch64-apple-darwin.tar.gz
   curl -LO https://github.com/tbreijm/brixms/releases/download/v0.1.0-alpha.3/brix-v0.1.0-alpha.3-aarch64-apple-darwin.tar.gz.sha256
   ```
2. Verify the checksum:
   ```bash
   shasum -a 256 -c brix-v0.1.0-alpha.3-aarch64-apple-darwin.tar.gz.sha256
   ```
3. Extract the archive (extracting the predictable top-level directory `brix-v0.1.0-alpha.3-aarch64-apple-darwin`):
   ```bash
   tar -xzf brix-v0.1.0-alpha.3-aarch64-apple-darwin.tar.gz
   ```
4. Enter the extracted top-level directory and verify the executable:
   ```bash
   cd brix-v0.1.0-alpha.3-aarch64-apple-darwin
   ./brix --version
   ```
   Outputs:
   ```text
   brix 0.1.0-alpha.3
   ```
5. Run preflight check and deliberation on the bundled external-input example:
   ```bash
   ./brix check examples/shipping-input.brix --input examples/shipping-input.json
   ./brix run examples/shipping-input.brix --input examples/shipping-input.json
   ```

#### Linux (`x86_64-unknown-linux-gnu`)

1. Download the release archive and SHA256 checksum file:
   ```bash
   curl -LO https://github.com/tbreijm/brixms/releases/download/v0.1.0-alpha.3/brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.tar.gz
   curl -LO https://github.com/tbreijm/brixms/releases/download/v0.1.0-alpha.3/brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.tar.gz.sha256
   ```
2. Verify the checksum:
   ```bash
   sha256sum -c brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.tar.gz.sha256
   ```
3. Extract the archive (extracting the predictable top-level directory `brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu`):
   ```bash
   tar -xzf brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.tar.gz
   ```
4. Enter the extracted top-level directory and verify the executable:
   ```bash
   cd brix-v0.1.0-alpha.3-x86_64-unknown-linux-gnu
   ./brix --version
   ```
   Outputs:
   ```text
   brix 0.1.0-alpha.3
   ```
5. Run preflight check and deliberation on the bundled external-input example:
   ```bash
   ./brix check examples/shipping-input.brix --input examples/shipping-input.json
   ./brix run examples/shipping-input.brix --input examples/shipping-input.json
   ```

### Building from source (alternative)

Install Rust through [rustup](https://rustup.rs/). The repository pins Rust
**1.96.1** in [`rust-toolchain.toml`](./rust-toolchain.toml), so the matching
toolchain is selected automatically.

```bash
git clone https://github.com/tbreijm/brixms.git
cd brixms
cargo test --workspace

# Type-check the checked-in identity example.
cargo run -p brix-cli -- check crates/brix-lower/tests/fixtures/id.brix
```

The final command prints:

```text
r : Int @Proven
```

### A small Brix program

```brix
config List<T> = Nil | Cons(T, List<T>)

fn head_or(xs: List<Int>, fallback: Int): Int = match xs {
  Nil => fallback
  Cons(head, _) => head
}

let answer: Int @Proven = head_or(Cons(42, Nil), 0)
```

The annotation is a contract, not documentation: the checker must establish
both the declared type and the requested evidence grade.

### Quickstart: End-to-end decision workflow (`examples/shipping.brix`)

The repository includes a complete finite-decision workflow in [`examples/shipping.brix`](./examples/shipping.brix):

```brix
config Decision = Expedite | Ship | Hold

rule stock() = 12
rule threshold() = 10

propose expedite(stock) priority 5 when stock >= 50 = Expedite
propose ship(stock, threshold) priority 10 when stock >= threshold = Ship
propose hold() priority 100 when true = Hold

commit shipping from (expedite, ship, hold)
show shipping
```

Run the pipeline from preflight check through execution, audit bundle creation, offline verification, and explanation:

```bash
# 1. Preflight check: parses, resolves imports, and validates the plan
cargo run -p brix-cli -- check examples/shipping.brix

# 2. Run deliberation to completion (committed at @Derived)
cargo run -p brix-cli -- run examples/shipping.brix

# 3. Deliberate, commit, and emit an ADR-0026 audit input bundle
cargo run -p brix-cli -- audit examples/shipping.brix --bundle /tmp/shipping.brixaudit --force

# 4. Verify the bundle independently against source (replaying and verifying separate @Audited receipts)
cargo run -p brix-cli -- verify \
  --expect-program 3a815590c807a8af7e7756d8f0edef99a4938e15830de282b24949fe88ba0d5e \
  examples/shipping.brix /tmp/shipping.brixaudit

# 5. Inspect why the winning candidate was selected
cargo run -p brix-cli -- why examples/shipping.brix --candidate ship

# 6. Inspect why another candidate was rejected
cargo run -p brix-cli -- whynot examples/shipping.brix --candidate expedite
```

In finite-decision deliberation, candidate selection commits at evidence grade **`@Derived`** during runtime execution, and the runtime decision remains **`@Derived`**. Successful independent replay issues and verifies separate **`@Audited`** audit receipts via `brix verify`.

### Quickstart: External-input decision workflow (`examples/shipping-input.brix`)

In `0.1.0-alpha.3`, BrixMS supports external operational inputs under the finite-decision profile ([ADR-0031](./spec/adr/ADR-0031_External_Input_Alpha.md)). In [`examples/shipping-input.brix`](./examples/shipping-input.brix), parameters previously hard-coded are declared as external inputs:

```brix
config Decision = Expedite | Ship | Hold

input stock: Int
input eligible: Bool
input region: Str

rule threshold() = 10
rule destination() = region
rule valid_destination(destination) = destination == "EU-NORTH"

rule can_ship(threshold, valid_destination) = match eligible {
  true => match valid_destination {
    true => stock >= threshold
    false => false
  }
  false => false
}

propose expedite() priority 5 when stock >= 50 = Expedite
propose ship(can_ship) priority 10 when can_ship == true = Ship
propose hold() priority 100 when true = Hold

commit shipping from (expedite, ship, hold)
show shipping
```

External values are supplied via strict JSON files conforming to schema `brix.input@1`, using unambiguous tagged scalar representations ([`examples/shipping-input.json`](./examples/shipping-input.json)):

```json
{
  "schema": "brix.input@1",
  "values": {
    "stock": {
      "type": "int",
      "value": "12"
    },
    "eligible": {
      "type": "bool",
      "value": true
    },
    "region": {
      "type": "string",
      "value": "EU-NORTH"
    }
  }
}
```

Key external input properties:
- **Strict Tagged JSON Scalars:** Schema `brix.input@1` requires explicit tagged scalar objects (`int` with decimal strings, `bool` with booleans, `string` with strings). Floats and lossy coercions are rejected. Any duplicate JSON key in the envelope, values, or objects is strictly rejected fail-closed.
- **Repeatable Disjoint Shards:** Multiple `--input` flags (e.g. `--input base.json --input overrides.json`) compose disjoint shards. Keys across shards must be mutually exclusive; overlapping keys fail closed with duplicate shard errors. Shard ordering does not affect canonical snapshot identity.
- **Snapshot & Context Identity:** Static program identity (`FiniteDecisionProgramId`) binds input declarations (name, type, ordinal), never input values. Supplied values are canonicalized into an `InputSnapshotId` (`Domain::Snapshot`). Deliberation `ContextId` binds both program identity and the active snapshot identity under `"brix.l3.finite-decision.context.input@1"`.
- **Derived Evidence:** External inputs enter execution strictly at epistemic grade **`@Derived`** (unverified external claims cannot ambiently upgrade to `@Audited` or `@Proven`). Candidate deliberation commits at **`@Derived`**, and the runtime decision remains **`@Derived`**.

Execute the full external-input lifecycle:

- **Prebuilt archive:** run `./brix <subcommand> ...` from the extracted directory.
- **Source workspace:** run `cargo run -p brix-cli -- <subcommand> ...` from the repository root.

The sequence below is shown with `./brix` (substitute `cargo run -p brix-cli --` if running from a source workspace):

```bash
# 1. Declaration-only check: validates syntax, imports, and plan contract without inputs
./brix check examples/shipping-input.brix
# (or from source workspace: cargo run -p brix-cli -- check examples/shipping-input.brix)
# Outputs: status: checked-input-contract, program: 44a5c10083cf9ebd7e948f2e4934087f39b4959bc2b32f085e98d20085ec7943

# 2. Preflight check with input: validates type matching, completeness, and dry-run deliberation
./brix check examples/shipping-input.brix --input examples/shipping-input.json

# 3. Deliberate to completion with input (@Derived)
./brix run examples/shipping-input.brix --input examples/shipping-input.json

# 4. Deliberate, commit, and emit an audit input bundle with bound input records
./brix audit examples/shipping-input.brix \
  --input examples/shipping-input.json \
  --bundle /tmp/shipping-input.brixaudit --force

# 5. Verify the bundle independently against source and caller-supplied inputs
# Offline replay re-derives the input snapshot and context, issuing and verifying separate @Audited receipts
./brix verify \
  --expect-program 44a5c10083cf9ebd7e948f2e4934087f39b4959bc2b32f085e98d20085ec7943 \
  examples/shipping-input.brix /tmp/shipping-input.brixaudit \
  --input examples/shipping-input.json

# 6. Inspect why the winning candidate was selected with external inputs
./brix why examples/shipping-input.brix \
  --input examples/shipping-input.json --candidate ship

# 7. Inspect why another candidate was rejected with external inputs
./brix whynot examples/shipping-input.brix \
  --input examples/shipping-input.json --candidate expedite
```

Multiple disjoint shards can be passed repeatably by specifying `--input` multiple times with non-overlapping keys (schematic example using non-repository placeholder paths):

```bash
# Schematic placeholder paths (disjoint shards composing the full input contract):
./brix run examples/shipping-input.brix \
  --input /path/to/shard-stock.json \
  --input /path/to/shard-region.json
```

### CLI guide

The `brix` CLI driver provides six file-oriented subcommands:

```text
brix check   <file.brix> [--input <path>...] [--json] [--package-path <dir>...]
             check a module; runs declaration-only check or preflight with --input

brix run     <file.brix> [--input <path>...] [--json] [--package-path <dir>...]
             execute a finite-decision deliberation plan to completion (@Derived)

brix audit   <file.brix> --bundle <out> [--input <path>...] [--force] [--json] [--package-path <dir>...]
             run and audit a finite-decision plan, emitting an audit input bundle on success

brix verify  --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>] [--input <path>...] [--json] [--package-path <dir>...]
             verify an audit input bundle against source and expected program pin (@Audited)

brix why     <file.brix> --candidate <name> [--input <path>...] [--json] [--package-path <dir>...]
             explain why a candidate was admitted or selected in deliberation

brix whynot  <file.brix> --candidate <name> [--input <path>...] [--json] [--package-path <dir>...]
             explain why a candidate was not admitted or not selected in deliberation
```

Global options: `--help` and `--version`.

Input options:
- `--input <path>` (or `--input=<path>`) can be repeated to supply disjoint input shards conforming to the strict `brix.input@1` JSON schema.
- `brix verify --profile l3-v1` rejects `--input` (exiting with usage error code 2), as external input shards apply to the `finite-decision` profile.

From a source workspace, prefix a command with `cargo run -p brix-cli --`, or build
the executable once with `cargo build -p brix-cli`. When using a prebuilt archive,
invoke `./brix` directly from the extracted directory.

### Reusable functions in decision programs (source build)

The working source adds pure, nonrecursive helpers to finite-decision programs
([ADR-0032](./spec/adr/ADR-0032_Finite_Decision_Functions.md)). This extension is
not included in the previously published alpha.3 archives.

```brix
fn enough(available: Int, needed: Int): Bool = available >= needed

input stock: Int
rule threshold() = 15
rule eligible(threshold) = enough(stock, threshold)

propose ship(eligible) priority 10 when eligible = stock
commit shipping from (ship)
```

Helpers can call other helpers and appear in lets, rules, and proposal guards
or values. They take their data explicitly as arguments: inputs, global lets,
and rule facts are not captured from the surrounding module. Arguments evaluate
once, left to right, including unused arguments; an arithmetic fault still
stops the decision.

Optional parameter and return annotations support `Int`, `Bool`, and `Str`,
with optional `@Derived`. These are checked against values at the call boundary.
Records and sum values can pass through unannotated parameters; composite type
annotations are not yet supported. Recursive calls and unsupported contracts
are rejected. Evaluation has nesting, work, and value-growth limits.

Run the full shipping example from this checkout:

```bash
cargo run -p brix-cli -- run examples/shipping-functions.brix \
  --input examples/shipping-functions.json
```

It selects `ship = Ship @Derived`. The same source and inputs work with
`check`, `why`, `whynot`, `audit`, and `verify` using the command forms above.
Helper bodies and contracts are included in the program pin; changing one
invalidates verification against the old pin. Successful replay produces
separate `Audited` receipts.

The library can also evaluate helper calls in `show` expressions. The CLI
currently removes `show` directives and prints its fixed decision report.

**CLI target surface & status:**
- `brix` implements exactly the six subcommands above.
- `brix verify` implements offline verification of ADR-0026 audit input transport bundles.
- `brix test`, `brix sim`, and interactive REPLs are deliberately out of scope and not implemented.

## What is coming

The next work is about completing the trust story and widening the useful
language surface, not replacing the architecture above.

### Near-term engineering

- discharge the remaining primitive typing relations so arithmetic,
  comparisons, and more matches can move from `Audited` to genuine `Proven`;
- widen offline audit bundle verification and transport beyond single-module
  finite-decision snapshots;
- add dependency tracking and incremental invalidation for type-realization
  results;
- extend the executable L3 subset beyond the current static rule-agenda and
  live finite-decision profiles;
- finish versioned context transport and confinement checks.

### Longer-term design and research

- parallelize deliberation while preserving the exact serial commit sequence;
- build broader native Brix packages and more realization regimes;
- complete the universal-world/faithfulness obligations without overstating
  the still-open mathematical claims;
- grow the language toward self-hosting while keeping the Rust kernels small
  and independently checkable.

The precise status is intentionally explicit:

- arithmetic and comparison currently top out at `Audited` where primitive
  leaves remain undischarged;
- catch-all matching is also deliberately capped;
- recursive functions are refused because functions are currently inlined;
- certified refutation does not exist yet, so negative results are conflicts or
  `Unknown`, never `Refuted`;
- context confinement and several durable artifact obligations remain partial.

The authoritative status ledger is
[`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md). The exact distinction
between implemented, test-pinned, and specified-only typing clauses lives in
[`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md).

## How the implementation is organized

BrixMS is a Rust workspace with nine focused crates:

```text
                         brix-canon
                             |
                       brix-semantic
                      /      |       \
              soc-core   brix-kernel  soc-regimes
                  |           ^           ^
                  |      brix-elaborate    |
                  |           ^           |
.brix -> brix-syntax -> brix-lower --------+
                           |
                        brix-cli
```

| Crate | Role |
| --- | --- |
| [`brix-canon`](./crates/brix-canon) | Canonical bytes, ordering, and digest identity |
| [`brix-semantic`](./crates/brix-semantic) | Shared artifacts, evidence grades, and legal publication routes |
| [`soc-core`](./crates/soc-core) | Settlement, incremental execution, journals, audit, and saturation |
| [`soc-regimes`](./crates/soc-regimes) | Literal and native Brix type-realization regimes |
| [`brix-kernel`](./crates/brix-kernel) | Independent proof-term acceptance and certificates |
| [`brix-elaborate`](./crates/brix-elaborate) | Checked bridge from audited evidence into the proof kernel |
| [`brix-syntax`](./crates/brix-syntax) | Surface AST, lexer, parser, and hostile-input bounds |
| [`brix-lower`](./crates/brix-lower) | Type-realization lowering and the executable L3 adapter |
| [`brix-cli`](./crates/brix-cli) | User-facing commands |

`brix-semantic` depends only on `brix-canon`, and `brix-kernel` depends only on
those two crates. This keeps the trusted proof boundary independent of the
parser, runtime, and regimes that construct proof candidates.

The former `brix-ast`/`brix-ir`/`brixc`/`brix-rt` engine served as a
differential oracle during the SOC transition and has been deleted. The current
workspace is the SOC-native implementation, not two competing engines.

## Repository guide

```text
crates/     the Rust implementation
spec/       constitution, decisions, semantic laws, contracts, and plans
docs/       the SOC foundation, language overview, and article material
vectors/    frozen canonical and certificate artifacts
packages/   experimental Brix package sources
scripts/    independent canonical, dependency, and traceability checks
```

For a conceptual introduction, read
[`docs/brix-language.md`](./docs/brix-language.md). For the governing design and
the exact boundary between claims and conjectures, continue with:

1. [`spec/README.md`](./spec/README.md) — document map and authority.
2. [`ADR-0002`](./spec/adr/ADR-0002_SOC_Constitution.md) — the accepted SOC
   constitution.
3. [`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md) — laws, executable
   anchors, and open obligations.
4. [`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md) — the
   native typing contract and its current limits.
5. [`Build_Plan_v3_SOC.md`](./spec/Build_Plan_v3_SOC.md) — dependency-ordered
   design plan; use the law registry and code for current landed status.

The mathematical source is
[`SOC_core_foundations_revised.tex`](./docs/SOC_core_foundations_revised.tex).
It labels established results, conditional claims, targets, and open questions
separately. The accepted engineering constitution governs where the documents
differ.

## Development and verification

The local merge bar is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/canon_crosscheck.py
python3 scripts/check_tcb_dependencies.py --check
scripts/test_tcb_dependency_gate.sh
python3 scripts/check_soc_law_map.py
scripts/test_law_map_provisional_gate.sh
```

An extracted release package directory can be smoke-validated end-to-end against an expected version:

```bash
python3 scripts/smoke_release_package.py <extracted_package_dir> <expected_version>
```

The required CI merge gates protecting `main` (defined in [`.github/workflows/ci.yml`](./.github/workflows/ci.yml)) are:

1. **`lint`**: formatting (`cargo fmt`), TCB dependency policy (`check_tcb_dependencies.py`), law-map traceability (`check_soc_law_map.py`), canon vector cross-check (`canon_crosscheck.py`), and Clippy warnings-as-errors;
2. **`build`**: workspace test archiving and doctests (`cargo test --doc --workspace`);
3. **`test`**: execution of the workspace test suite via `cargo nextest`;
4. **`determinism`**: repeated test execution asserting zero git status drift on frozen artifacts (proxy for G3 reproducibility);
5. **`conformance`**: dedicated regime test gate covering `soc-regimes` native type-checker parity;
6. **`acceptance`**: adversarial certificate vector verification in `brix-kernel`;
7. **`reproducibility`**: reproducible emit, cache integrity, and deterministic size budgets in `soc-core`;
8. **`cargo-deny`**: supply-chain security, license compatibility, crate bans, and advisory checks.

`unsafe` is denied workspace-wide, and unordered standard hash maps are denied in semantic paths.

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the determinism discipline,
dependency policy, and specification-erratum workflow.

## License

BrixMS is licensed under [Apache-2.0](./LICENSE).
