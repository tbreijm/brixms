# Alpha.9 request evidence profile

A9-4 adds explicit request evidence authority to finite-decision execution and
portable audit replay. It builds on #52's authority fence, #59's V2 context
validator and the A9-3 trusted-host channel. It leaves the existing runtime
`context` identity and frozen V1 audit encodings unchanged.

## Host inputs and evidence inputs

A trusted host constructs immutable `RequestEvidencePolicy` values. Each
policy independently pins the finite-decision program, runtime claim IDs and
optional documentary snapshot digest, plus the supported verifier/profile,
namespace, tenant, principal and resource ceilings. The exact versioned JSON
shape is documented in the [Python SDK README](../../bindings/python/README.md).
All fields and limits are explicit; unknown versions, unknown fields, duplicate
keys and attempts to widen hard ceilings are rejected.

The operation lowers the actual source, compares its program identity with the
host expectation, constructs a canonical ContextModelV2, resolves the actual
program preimage and runs the V2 validator before admitting any evidence. The
private checked request context binds the full policy through a canonical
capability-view digest. Copying its ID does not grant authority.

Evidence arrives through separate input envelopes. Runtime evidence must match
an independently pinned complete target ID and pass the existing bounded replay
verifier before it can enter the registry. Every pinned runtime claim must be
present. Documentary entries must produce the independently pinned snapshot;
missing evidence, cross-context evidence, forged bytes and exhausted budgets
fail closed. Envelope assertions cannot choose their own expected pins.

## Invocation paths

- CLI: `brix run ... --request-context host-policy.json --json`, with the same
  option on `why`, `whynot`, `audit` and `verify`. The policy file belongs to
  the invoking host, separate from `--input` evidence files.
- Stdio: only `serve --stdio --trusted-host` accepts a per-request
  `params.request_context` policy on those operations. The embedding host owns
  this channel and authenticates callers before choosing a policy and inputs.
- Python: `BrixClient(trusted_host=True)` accepts immutable
  `RequestEvidencePolicy` values through each operation's `request_context=`
  argument. Different requests on the same client can use different policies.
  Neither a previous call nor ambient environment supplies a default policy.

No environment mutation, thread-local authority default or mutable context
registry participates in a server request. Nested evaluation and explanations
use the operation's admitted input registry. Result JSON adds `request_context`
for a validated host request, keeping the existing `context` field's runtime
meaning. Cancellation/timeouts do not transfer authority to a later request.

## Portable audits and legacy behavior

A host-context audit emits `brix.audit-request-evidence@1`, an additive wrapper
containing exact V1 audit bytes, validated V2 request-context bytes/identity, and
ordered input/evidence contents. Payload identities and wrapper identity use
brix-canon. Files use synthetic transport labels rather than original paths.
The V1 bundle format, encoding and identity remain unchanged.

Verification independently authorizes the host policy against actual source and
compares exact context bytes and identity before decoding or materializing the
embedded inputs. It checks wrapper and payload identities, reconstructs the
snapshot under the same budgets, replays evidence, and calls the existing V1
checker. Replay needs the source and host policy, but does not need the original
input files or a live evidence-producing world. Temporary files use private
0700 directories and 0600 files on Unix and are cleaned up by ownership guards.

Standalone CLI invocation without `--request-context` snapshots the two legacy
`BRIX_EXPECT_*` variables once. Legacy `check` and `test` retain their CLI
behavior. Those invocations retain raw V1 artifacts and do not acquire a V2
request-context label. Default stdio uses an explicit evidence-free context and
ignores ambient pins. It still supports ordinary evidence-free V1 audit replay.
A strict host-context verifier rejects unbound legacy V1 artifacts.

This profile covers finite-decision request evidence. L3-v1 verification keeps
its legacy contract and rejects this policy. World evidence APIs already use
their independent complete target expectations; this finite-decision policy is
not accepted as a replacement. Scoped named queries retain their distinct
A9-3 partition profile and host policy.
