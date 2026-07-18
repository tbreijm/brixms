# Training data contract

Place compiler-validated `train.jsonl`, `valid.jsonl`, and `test.jsonl` in
`data/brix-builder/`. They are intentionally not checked in or synthesized by
the runtime: a fluent but unaudited model transcript is not training truth.

Use MLX LM chat records. Each assistant message contains exactly one action from
`brix-builder schema`; tool evidence is represented as the following user
message. Keep the hidden package-level evaluation set separate from these files.

```json
{"messages":[{"role":"system","content":"..."},{"role":"user","content":"PACKAGE_BRIEF: ..."},{"role":"assistant","content":"{\"action\":\"check_candidate\",\"reason\":\"validate the repair\"}"},{"role":"user","content":"TOOL_RESULT E0004: {\"ok\":true,\"status\":\"passed\",\"data\":{...}}"}]}
```

Before admitting an example, replay the candidate through check, canonical
formatting, tests, quality, semantic diff, and package build. Preserve negative
examples and structured repair trajectories; do not train on a claimed success
whose mandatory evidence is unavailable.
