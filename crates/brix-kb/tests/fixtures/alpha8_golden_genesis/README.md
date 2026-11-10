# Frozen alpha.8 golden genesis

This empty executable world was produced with the alpha.8 CLI so the R0 golden
hash tests can replay their fixed workload with the original execution-profile
identity. The tests copy this genesis into a temporary directory and append
the deterministic batches; they do not relabel an alpha.9 world.

Producer:

- CLI version: `0.1.0-alpha.8`
- Binary: `/private/tmp/brix-a9-6-target/debug/brix`
- SHA-256: `02001da03ef552e2a94bce68842f01d883256faa31893d50d61171bf685bff9b`
- Command: `brix world init <fixture-dir> <fixture-dir>/root.brix --json`
- World/manifest digest: `9b0b726268404f207fb66b704dad58ad2c6c5022e7553bfcc7f4bf0b1abd6557`
- Program digest: `3d354cc684b29b5d6c5d71fad12156ba5e36cea9addc8b955004d3cda3ca6132`
- Genesis revision digest: `5fb2a7e4f158c0bdd445695c48c4a54e5e55442d67481ff37627c70a1d7b887b`

`root.brix` and the embedded source closure match `r0_chunked_persist.rs::MODEL`
byte for byte (1003 bytes, including its leading newline). The
fixture intentionally contains only revision zero and no facts. Its expected
revision-history hashes remain pinned in that test; they are not regenerated
from this candidate runtime.
