# Brix CLI quality profiles

This document defines the executable quality rules in the current `brix-cli`.
It is an implementation contract for the compiler facts available today, not a
replacement for BrixMS v9 Part VIII §§8–10.

Every profile includes all rules from profiles above it. Rule order and IDs are
stable machine-facing evidence.

| Profile | Added required rules |
|---|---|
| `prototype` | `compiler.validity` |
| `standard` | `source.canonical_format`, `package.identity`, `compiler.semantic_coverage` |
| `production` | `package.explicit_manifest`, `test.execution`, `architecture.ownership`, `architecture.capabilities` |
| `critical` | `test.mutation`, `conformance.result`, `supply_chain.signatures` |

Rule states are `passed`, `failed`, or `unavailable`. A gate passes only when
every required rule passes. An evaluated violation makes the aggregate status
`failed`; otherwise any unavailable required fact makes it `unavailable`.

## Grounding

- `compiler.validity` runs the public compiler parse, lowering, type/effect,
  and phase checks.
- `source.canonical_format` compares the source with the canonical AST
  formatter output.
- `package.identity` uses the parsed source declaration and
  `Manifest::check_matches_source_decl`.
- `compiler.semantic_coverage` reads compiler diagnostic `BRX-LOW-0002`.
  A skipped scenario is covered only when the public `brix test` classifier
  confirms its exact shape is executable. Unsupported scenarios and every
  other declaration skipped by lowering make this fact unavailable.
- `package.explicit_manifest` checks for an on-disk `brix.toml`; synthesized
  metadata is insufficient for production.

The remaining production and critical facts are explicitly unavailable to the
quality evaluator because it has no bound test-run record or exported test
relations, resolved ownership/capability analysis, mutation results, package
conformance results, or verified provenance/SBOM/signature results. The public
test oracle can execute its documented scenario subset, but that evidence is not
yet persisted and bound into a quality evaluation. These rules therefore cannot
pass.

## Aggregate diagnostic codes

| Code | Meaning |
|---|---|
| `BRX-QUALITY-0000` | all required rules passed |
| `BRX-QUALITY-0002` | at least one required rule failed |
| `BRX-QUALITY-0003` | no rule failed, but required evidence is unavailable |

Each aggregate diagnostic contains the profile, aggregate status, and every
required rule's ID, minimum profile, status, and detail. Maps use deterministic
key ordering and rules retain the table order above.
