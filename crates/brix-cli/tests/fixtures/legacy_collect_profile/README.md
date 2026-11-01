# Frozen legacy collect audit

Captured once with the reviewed `16b7752` foundation, before A9-6, using its
`brix-world-net@1` evaluator and the accompanying source. Four rows have equal
sort values and keys `aa`, `b`, `ccc`, `z`. The recorded winning list is
`b`, `z`, `ccc`, `aa`, which differs from typed UTF-8 ordering.

The audit bytes and independent expected head/program pins are retained to
verify legacy replay after introducing evaluator @2. This is synthetic data;
it is not an Atlas customer model. Do not regenerate the expected bytes to
accommodate a verifier regression.
