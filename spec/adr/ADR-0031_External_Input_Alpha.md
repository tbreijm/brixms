# ADR-0031 — External Inputs Alpha: Strict Schemas, Bounded Shards, and Deterministic Context Identity

Status: **Accepted** (2026-09-06). Governs the external input contract for BrixMS `0.1.0-alpha.3`
(Wave 1) under `brix.l3.finite-decision@1`.

Date: 2026-09-06.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§4.1 epistemic
lattice, §5.3 fail closed, §8.1 commitment/deliberation split, §9 calendar and interning),
[ADR-0010: SOC Language Design](./ADR-0010_SOC_Language_Design.md),
[ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md),
[ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md),
[ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md),
[ADR-0022: Source Re-Derived Manifests](./ADR-0022_Source_Re_Derived_Manifests.md) (§5 re-derivation doctrine),
[ADR-0026: The Audit-Input Transport Bundle](./ADR-0026_Audit_Input_Transport_Bundle.md) (§6 bounded decode, §9 hard boundaries),
[ADR-0030: Finite-Decision Alpha](./ADR-0030_Finite_Decision_Alpha.md) (⟨D-PROFILE⟩, ⟨D-GRAMMAR⟩, ⟨D-PROGID⟩, ⟨D-EVIDENCE⟩).

---

## 1. Context and Problem Statement

BrixMS `0.1.0-alpha.2` (ADR-0030) delivered the executable finite-decision profile (`brix.l3.finite-decision@1`),
supporting derived settlement rules (`rule`), candidate proposals (`propose`), and deterministic
frontier deliberation (`commit`). In alpha.2, all inputs to a decision module are closed, in-source
constructs: rules derive facts from earlier rules, and static constants are declared with `let`.

Real-world decision systems require binding operational parameters from outside the source program
(such as inventory levels, customer tiers, market indices, or external sensor thresholds) without
mutating code or recompiling modules. However, admitting external inputs introduces significant
security, determinism, and epistemic boundaries:

1. **Hostile Input & Resource Exhaustion:** External files are untrusted data. Decoders that read
   unbounded streams, deserialize into uncontrolled memory structures, or rely on standard JSON
   libraries risk memory exhaustion, integer overflow, and stack overflow.
2. **Silent State Mutation via Duplicate Keys:** Standard JSON specifications permit duplicate keys
   or leave duplicate resolution undefined. Standard deserializers (such as `serde_json`'s default
   object visitor) silently overwrite earlier keys with later ones (last-write-wins), enabling
   adversarial shadowing and non-deterministic behavior.
3. **Identity Collapse & Trust Leaks:** If program identity were to hash supplied input values,
   every execution with different inputs would become a distinct program, destroying rule verification
   and caching. Conversely, if context identity failed to bind the exact input snapshot, distinct
   executions would collide in the journal.
4. **Epistemic Over-Claiming:** External data supplied on the command line or over transport carries
   no mathematical proof. Admitting supplied data at `Audited` or `Proven` would violate the SOC
   epistemic lattice (ADR-0002 §4.1, ADR-0016) and create an unchecked publication path.

This ADR defines the **alpha.3 external input contract** (Wave 1): a minimal, strictly bounded,
fail-closed external input architecture preserving byte-for-byte backward compatibility with alpha.2.

---

## 2. Decision Points

### ⟨D-GRAMMAR⟩ Source Declaration Grammar: `input NAME: TYPE`

External inputs are declared at top level using a dedicated no-semicolon, newline-delimited syntax:
```brix
input <name>: <type>
```

1. **Top-Level Item:** `input` is a first-class top-level declaration in `ast::Module`.
2. **No Semicolon:** Conforms to the universal Brix surface grammar (ADR-0010, ADR-0030); trailing
   semicolons are forbidden.
3. **Collision Discipline:** The declared `<name>` participates in top-level identifier collision
   rules. An `input` declaration sharing a name with another `input` declaration is rejected at plan lowering
   with `DuplicateInputName`; sharing a name with any other top-level item (`config`, `let`, `rule`, `propose`)
   is rejected with `DuplicateItemName`.
4. **Scope Participation:** Declared input names are bound in the expression scope for subsequent
   rule bodies, candidate proposal guards, proposal values, and show expressions. Referencing an
   undeclared identifier remains an immediate lowering error (`UnresolvedReference`).

### ⟨D-TYPES⟩ Exact Scalar Type Support

Initial external input support is restricted to the scalar types that the existing finite-decision
expression and value layer (`L3ValueV2`, `L3ValueType`) can faithfully represent:

| Surface Type | Lowered Value Type | Runtime Representation | Value Domain |
|---|---|---|---|
| `Int` | `L3ValueType::Int` | `L3ValueV2::Int(i64)` | Signed 64-bit integer (`-9223372036854775808..=9223372036854775807`) |
| `Bool` | `L3ValueType::Bool` | `L3ValueV2::Bool(bool)` | Boolean (`true` or `false`) |
| `Str` | `L3ValueType::Str` | `L3ValueV2::Str(String)` | Valid UTF-8 string scalar sequence |

**Zero Floating-Point & Zero Coercion Policy:**
- Floats (`Float`) are rejected. Per ADR-0027 §9 and CONTRIBUTING.md, floats are excluded from semantic
  decision paths.
- Lossy coercions (e.g. parsing `"123"` as an integer when declared as `Str`, or interpreting non-zero
  integers as booleans) are strictly forbidden. Any type disagreement between declaration and artifact
  is a fail-closed error.
- Non-scalar types (records, sum variants) are not supported as external inputs in Wave 1 and are
  rejected at declaration lowering.

### ⟨D-SCHEMA⟩ Strict External Artifact Schema: `brix.input@1`

External inputs are transported as strict JSON artifacts conforming to schema `brix.input@1`:

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

1. **Envelope:** Top level MUST be a JSON object containing exactly `"schema"` and `"values"`.
2. **Schema Identifier:** The `"schema"` field MUST be the exact string `"brix.input@1"`.
3. **Strict Top-Level Rejection:** In adherence to repo strict-decoding policy, any unknown top-level
   field is a fatal decoding rejection.
4. **CLI Tagged Value Compatibility:** Values within `"values"` use the unambiguous tagged representation
   matching CLI `TaggedValue`:
   - `int`: `{"type": "int", "value": "<decimal-string>"}` — string-encoded decimal representation
     preventing IEEE 754 precision loss during transport.
   - `bool`: `{"type": "bool", "value": true | false}` — native JSON boolean.
   - `string`: `{"type": "string", "value": "<string>"}` — native JSON string.
5. **Inner Strictness:** Any unknown property inside a tagged value object (e.g. `{"type": "int", "value": "1", "extra": 2}`)
   is rejected immediately.

### ⟨D-NODUPKEYS⟩ Strict Rejection of Duplicate JSON Keys

Standard JSON maps silently resolve duplicate keys via last-write-wins, hiding input tampering and
ambiguity. This specification requires strict zero-tolerance duplicate key rejection:

1. **Envelope Duplicates:** Duplicate `"schema"` or `"values"` keys in the root object are rejected.
2. **Values Duplicates:** Duplicate input names within the `"values"` object are rejected.
3. **Tagged Value Duplicates:** Duplicate `"type"` or `"value"` keys within a value object are rejected.
4. **No Deserialization to Map:** Decoders MUST NOT deserialize into a standard `HashMap`, `BTreeMap`,
   or `serde_json::Value` prior to duplicate detection. Duplicate detection is performed by a dedicated
   parser that tracks keys during object construction and fails closed upon any repetition.

### ⟨D-BOUNDS⟩ Bounded Resource Enforcement

In accordance with ADR-0022 §5 and ADR-0026 §6, resource bounds are strictly enforced before unbounded allocation or CPU amplification:

| Constant | Value | Enforcement Point |
|---|---|---|
| `MAX_INPUT_FILE_BYTES` | 1 MiB (1,048,576 B) | Checked on open handle metadata, then strictly enforced during read with regular-file bounded reader `take(limit + 1)` |
| `MAX_INPUT_FILES` | 16 files | Enforced before opening or iterating input shard files |
| `MAX_AGGREGATE_BYTES` | 4 MiB (4,194,304 B) | Preflighted on metadata with `checked_add` before shard reads, and rechecked authoritatively on actual read bytes |
| `MAX_INPUT_COUNT` | 256 inputs | Checked iteratively as input entries are decoded and merged |
| `MAX_INPUT_NAME_BYTES` | 64 bytes | Enforced while scanning identifier before allocation exceeds bound; rejected during source lowering if declaration exceeds 64 bytes |
| `MAX_STRING_VALUE_BYTES` | 64 KiB (65,536 B) | Checked during string value decoding |
| Integer Domain | `i64::MIN..=i64::MAX` | Parsed strictly as base-10 signed integer (decimal string ONLY); leading zeros and overflow/underflow rejected |

Decoders must fail closed with typed errors. Hostile inputs (e.g. oversized streams, deeply nested structures, trailing garbage after the closing brace, non-regular files) must never trigger panic or unbounded read/allocation.

### ⟨D-SHARDS⟩ Disjoint Shard Composition

Multiple input files (e.g. `--input base.json --input overrides.json`) represent **disjoint shards**:

1. **Disjointness Requirement:** Every input shard must declare mutually exclusive input names.
   If an input name appears in more than one shard file, merging fails closed with a duplicate shard
   key error. Shards do not overlay or shadow one another.
2. **Order Independence:** The canonical snapshot and its cryptographic identity are independent of
   shard ordering. Merging `[shard_A, shard_B]` produces the exact same snapshot and identity as
   `[shard_B, shard_A]`.
3. **Execution Completeness:** For module execution, the aggregate snapshot must exactly match the
   module's input declarations:
   - **Extra Inputs:** Supplying an input not declared in `.brix` is an execution rejection.
   - **Missing Inputs:** Failing to supply a declared input is an execution rejection.
   - **Type Mismatch:** Supplying a value whose type differs from the declared type is an execution rejection.

### ⟨D-CHECK⟩ Separable Declaration and Completeness Validation

To support flexible tooling workflows, validation is factored into two separable phases:

1. **Declaration Validation (`validate_against_declarations`):**
   Validates that all supplied inputs in the snapshot correspond to declared inputs with matching types.
   Permits partial/incomplete snapshots (does not require all declared inputs to be present).
2. **Completeness Validation (`validate_completeness`):**
   Enforces total coverage for execution: requires valid declaration alignment AND asserts that every
   declared input has an admitted value.

`brix check <file.brix>` without `--input` validates syntax and declarations. `brix check <file.brix> --input <file.json>`
validates complete snapshot fulfillment.

### ⟨D-IDENTITY⟩ Program Identity vs. Snapshot/Context Identity

External inputs separate static program specification from runtime dynamic environment:

1. **Program Identity (`FiniteDecisionProgramId`):**
   Binds input **declarations** (normalized name, type, and declaration ordinal), but NEVER input values.
   A module's program identity is stable regardless of external data values supplied at runtime.
2. **Snapshot Identity (`InputSnapshotId`):**
   A dedicated content-addressed identifier uniquely binding the exact sorted, canonicalized set of
   supplied values:
   $$\text{InputSnapshotId} = \text{Digest}(\text{Domain::Snapshot}, \text{preimage})$$
   $$\text{preimage} = \text{write\_tag}(\text{"brix.input.snapshot@1"}) \mathbin{\Vert} \text{write\_uint}(n) \mathbin{\Vert} \prod_{i=1}^n (\text{write\_ident}(k_i) \mathbin{\Vert} \text{canon\_write}(v_i))$$
   Sorted strictly by canonical NFC-normalized name bytes.
3. **Context Identity (`ContextId`):**
   The deliberation context identity binds the program, initial world, policy, and the active snapshot:
   When inputs are present, the context incorporates `InputSnapshotId` under the documented tag
   `"brix.l3.finite-decision.context.input@1"`.
   When no inputs are declared or supplied, the context encoding remains 100% byte-identical to alpha.2.

### ⟨D-EVIDENCE⟩ Epistemic Lattice & Verification Re-Derivation

External input values reside strictly at epistemic grade **`Derived`** (ADR-0002 §4.1, ADR-0016):

1. **Never Ambiently Audited:** External values provided by an operator or external system are
   unverified assertions. They are admitted into execution at grade `Derived`. Supplying an artifact
   does not earn `Audited` or `Proven`.
2. **Audit Replay Doctrine:** In accordance with ADR-0022 and ADR-0026, audit verification
   (`brix verify`) must re-derive the input snapshot and context from caller-supplied input files.
   Audit verifiers MUST NEVER trust a bundle-supplied snapshot or unverified claim.

### ⟨D-COMPAT⟩ Alpha.2 Preservation & Backward Compatibility

Existing alpha.2 programs without input declarations and CLI runs without `--input` flags remain
completely unaffected:

1. **Zero-Input Identity Preservation:** If `plan.inputs` is empty, `finite_decision_program_preimage`
   does not emit any input frame, producing the exact byte stream and `ProgramId` as alpha.2.
2. **Unchanged Shipping Workflow:** `examples/shipping.brix` contains no input declarations and
   remains byte-for-byte identical, passing all existing tests without modification.
3. **Additive Surface:** All external input data structures, decoders, and AST variants are strictly
   additive.

---

## 3. Architecture Overview

```text
       Input Files (.json)                           .brix Source
    [Shard 1]     [Shard 2]                               |
        |             |                                   v
        v             v                            [brix-syntax]
  +-------------------------+                      Parser (bounded)
  | Strict Bounded Decoder  |                             |
  |  - Bounded file bytes   |                             v
  |  - No duplicate keys    |                     AST (with InputDecl)
  |  - Tagged value parsing |                             |
  +-------------------------+                             v
        |             |                              [brix-lower]
        v             v                     lower_finite_decision_plan
    InputShard    InputShard                              |
        \             /                                   |
         \           /                                    +-----------------------+
          v         v                                     |                       |
    [canonicalize_shards]                                 v                       v
    - Disjointness check (no dupes)               FiniteDecisionPlan     FiniteDecisionProgramId
    - Aggregate bounds check                       (Declares inputs)     (Binds names + types,
    - Lexicographical sort by NFC name                    |               NEVER values)
          |                                               |                       |
          v                                               |                       |
    InputSnapshot                                         |                       |
    (SnapshotId: Domain::Snapshot)                        |                       |
          |                                               |                       |
          +-------------------+---------------------------+                       |
                              |                                                   |
                              v                                                   |
                   validate_completeness                                          |
                   - Types match declarations                                     |
                   - No unknown inputs                                            |
                   - No missing inputs                                            |
                              |                                                   |
                              v                                                   |
                     [brix-lower runtime]                                         |
                     Builds ContextId <-------------------------------------------+
                     (Binds ProgramId + InputSnapshotId)
                              |
                              v
                      Frontier Deliberation
                   (Inputs readable as @Derived)
```

---

## 4. Implementation Mapping

| Specification Requirement | Implementation Anchor |
|---|---|
| Surface grammar `input NAME: TYPE` | `crates/brix-syntax/src/ast.rs` (`InputDecl`), `lexer.rs`, `parser.rs` |
| AST & Module representation | `brix_syntax::ast::Item::Input(InputDecl)` |
| Plan lowering & scope bindings | `crates/brix-lower/src/finite_decision/plan.rs` (`FiniteDecisionInput`) |
| Duplicate name collision checking | `lower_finite_decision_plan` (`DuplicateInputName`, `DuplicateItemName`) |
| Source input name bound checking | `lower_finite_decision_plan` (`InputNameTooLong`) |
| Expression referencing declared inputs | `lower_expr_v2` via visible bindings in `plan.rs` |
| Program identity binding declarations | `finite_decision_program_preimage` (`brix.l3.finite-decision.inputs@1`) |
| Strict JSON schema `brix.input@1` decoder | `crates/brix-lower/src/input.rs` (`decode_input_shard`) |
| Duplicate key rejection (envelope & inner) | `crates/brix-lower/src/input.rs` (`StrictJsonParser`) |
| Pre-allocation resource bounds | `crates/brix-lower/src/input.rs` (`InputLimits`) |
| Bounded regular-file reader helper & structured I/O errors | `crates/brix-lower/src/input.rs` (`decode_input_shard_from_file`, `InputDecodeError::IoError { path, message }`) |
| Whole-set preflight & loading helper | `crates/brix-lower/src/input.rs` (`load_input_snapshot_from_paths`) |
| Disjoint shard merger & canonicalization | `crates/brix-lower/src/input.rs` (`canonicalize_input_shards`) |
| Canonical snapshot representation & ID | `crates/brix-lower/src/input.rs` (`InputSnapshot`, `InputSnapshotId`) |
| Separable declaration/completeness checks | `InputSnapshot::validate_against_declarations`, `validate_completeness` |
| Fallible runtime construction with inputs | `crates/brix-lower/src/finite_decision/runtime.rs` (`build_with_inputs`, `FiniteDecisionBuildError`) |
| Bound input record at epistemic `@Derived` | `crates/brix-lower/src/finite_decision/runtime.rs` (`BoundInputRecord`) |
| Deliberation run context & inputs binding | `crates/brix-lower/src/finite_decision/runtime.rs` (`FiniteDecisionRun.context`, `FiniteDecisionRun.inputs`) |
| Input injection into evaluation environment | `crates/brix-lower/src/l3_v2.rs` (`EvalEnv::inputs`, input lookup in `LetRef`) |
| Show expression evaluation against inputs/facts with deliberation integrity | `crates/brix-lower/src/finite_decision/runtime.rs` (`FiniteDecisionRuntime::evaluate_shows` verifies fresh deliberation integrity against tampering) |
| Input-aware audit environment & helper | `crates/brix-lower/src/finite_decision/runtime.rs` (`finite_decision_audit_environment_from_plan_with_inputs`) |
| Input-aware audit bundle verification | `crates/brix-lower/src/audit_bundle.rs` (`check_finite_decision_audit_input_bundle_from_module_with_inputs_v1`, `check_finite_decision_audit_input_bundle_from_source_with_inputs_v1`) |
| Standardized `--input` / `--input=` parsing | `crates/brix-cli/src/cli.rs` (`try_parse_input_flag` across `check`, `run`, `audit`, `verify`, `why`, `whynot` with option-boundary check and joined dash-path support) |
| Centralized snapshot loader, stable diagnostic codes & injection-safe rendering | `crates/brix-cli/src/commands/mod.rs` (`load_cli_input_snapshot`, `bound_input_to_json`, `CliInputError` diagnostic mapping, `fmt_value_human` escaping) |
| Declaration-only contract check & preflight | `crates/brix-cli/src/commands/check.rs` (`execute_check`, status `checked-input-contract`) |
| CLI run deliberation with external inputs | `crates/brix-cli/src/commands/run.rs` (`execute_run`) |
| CLI audit bundle production with inputs | `crates/brix-cli/src/commands/audit.rs` (`execute_audit`, emits `inputs: inputs_json` on success) |
| CLI audit bundle verification with inputs | `crates/brix-cli/src/commands/verify.rs` (`execute_verify`) |
| CLI why / whynot input explanation | `crates/brix-cli/src/commands/why.rs` (`execute_why_or_whynot`) |
| Additive JSON schema fields under `brix.cli.result@1` | `crates/brix-cli/src/json.rs` (`CliResultJson.input_snapshot`, `CliResultJson.inputs`, `InputJson`) |
| Alpha.2 backward compatibility | Untouched `examples/shipping.brix`, conditional preimage emission, zero-input 12-field JSON outputs |

---

## 5. Status & Compatibility

- **Status:** Complete (Wave 1, Wave 2, and Wave 3 delivered, audit corrections integrated).
- **Scope:** Complete implementation across all three waves:
  - **Wave 1:** Surface grammar `input NAME: TYPE`, bounded AST representation, strict schema `brix.input@1` decoder, duplicate key rejection via `StrictJsonParser`, pre-allocation resource bounds (`InputLimits`), bounded file reader with structured I/O errors (`load_input_snapshot_from_paths`, `InputDecodeError::IoError`), canonical disjoint shard canonicalization (`canonicalize_input_shards`), and content-addressed cryptographic identities (`InputSnapshotId`, `input_context_id`).
  - **Wave 2:** Fallible runtime construction (`FiniteDecisionRuntime::build_with_inputs`), fail-closed declaration and completeness validation (`InputSnapshot::validate_completeness`), bound input records at epistemic `@Derived`, execution environment injection (`EvalEnv::inputs`), show evaluation with deliberation integrity verification against fact tampering, and input-aware audit environment consolidation.
  - **Wave 3:** CLI integration with standardized `--input <path>` / `--input=<path>` parsing across all six commands (`check`, `run`, `audit`, `verify`, `why`, `whynot`), centralized snapshot loading and error mapping with stable diagnostic codes (`CliInputError`), injection-safe and deterministic human value rendering (`fmt_value_human`), declaration-only contract checking (`status: checked-input-contract`), input-aware audit bundle generation and verification re-derivation, profile-disciplined rejection of `--input` under `verify --profile l3-v1`, and additive JSON serialization preserving 12-field zero-input shapes.
- **Compatibility:** Strictly additive and backward compatible with `0.1.0-alpha.2`. Zero-input plans continue to construct, deliberate, audit, verify, and emit byte-for-byte identical context and program identities. All existing vectors and tests continue to pass identically.
