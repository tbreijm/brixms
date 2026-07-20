# Ring 1 stdlib package status

| Package | Content | State |
|---|---|---|
| [`brix.type`](brix.type) | shadow-mode typefacts (slices 1–2) | Real implementation |
| [`brix.math`](brix.math) | Int/Float overloads + `approxEq` | Slice 1 landed |
| [`brix.core`](brix.core) | `Id`, `id_of`/`id_eq`, overloaded `identity` | Slice 1 landed |
| [`brix.time`](brix.time) | Instant/Duration as `Int` helpers (`instant_of`, `add`, `since`, …) | Slice 1 landed (aliases blocked) |
| [`brix.rel`](brix.rel) | empty scaffold | Next overnight target |

## Notes

- Typed overloads work (compiler fix on this branch).
- `type Instant = Int` still mismatches — time uses documented `Int` seconds until nominal newtypes land.
- `Decimal` / units / `%` still Ring 0 gaps for math.
