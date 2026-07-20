# Ring 1 stdlib package status

| Package | Content | State |
|---|---|---|
| [`brix.type`](brix.type) | shadow-mode typefacts (slices 1–2) | Real implementation |
| [`brix.math`](brix.math) | Int/Float overloads + `approxEq` | Slice 1 landed |
| [`brix.core`](brix.core) | `Id`, `id_of`/`id_eq`, overloaded `identity` | Slice 1 landed |
| [`brix.time`](brix.time) | Instant/Duration as `Int` helpers (`instant_of`, `add`, `since`, …) | Slice 1 landed (aliases blocked) |
| [`brix.rel`](brix.rel) | empty scaffold | Next overnight target |

## Layout note

Multi-file packages (issue #42) are unblocked: `src/world.brix` remains the
required entry — it alone carries `package NAME @ VERSION` — but any sibling
`src/<name>.brix` file is a real submodule, published under the
package-qualified path `pkg.<name>` (e.g. `src/order.brix` → `brix.math.order`,
reachable as `use brix.math.order.{…}` from another package or bare
`order.min(...)` / auto-imported bare names inside the same package). All of
`check`/`fmt`/`test`/`quality`/`build` load the same whole-package graph, so a
submodule's coverage, formatting, and diagnostics are exactly as load-bearing
as the entry's. Reordering `.brix` files on disk never changes the result.
Nested directories (`src/units/world.brix`) are out of scope for this slice —
one flat `src/` per package.

## Notes

- Typed overloads work (compiler fix on this branch).
- `type Instant = Int` still mismatches — time uses documented `Int` seconds until nominal newtypes land.
- `Decimal` / units / `%` still Ring 0 gaps for math.
