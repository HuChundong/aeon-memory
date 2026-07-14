# TypeScript differential baseline

The oracle is commit `4339e63650920871eb0e8888083a1779d114e3ae`.
It is always checked out outside this repository and passed explicitly as
`AEON_MEMORY_TS_BASELINE`; no sibling path or foreign Git object is assumed.

Reproducible bootstrap used on 2026-07-13:

```sh
BASE="$(scripts/prepare-ts-baseline.sh)"
AEON_MEMORY_TS_BASELINE="$BASE" node \
  crates/aeon-memory-core/tests/fixtures/gen-golden-fixtures.mjs
AEON_MEMORY_TS_BASELINE="$BASE" node \
  crates/aeon-memory-core/tests/fixtures/gen-offload-legacy-replay.mjs
AEON_MEMORY_TS_BASELINE="$BASE" node \
  crates/aeon-memory-core/tests/fixtures/gen-ts-runtime-trace.mjs
```

Evidence from that run:

- source-tree hash before and after dependency bootstrap and oracle execution:
  `9efff8e246d52bb96353865efc66a79888b5136e950a5bbba400ad2bae3837ec`
- archived `package.json` SHA-256:
  `5860b90231bfd7b50ac1967e71f79d9198c593bb7a5566be5c108f24f9d52307`
- repository dependency snapshot `package-lock.json` SHA-256:
  `4ffbb116aa56fb46c9af791ff166486142cf884271c99a767ff145429afa2539`
- `npm ls --depth=0 --json` SHA-256:
  `42dd41cdad206b68a3ed91603551140d5da8bdd88808469f817b1a3ab87f5081`

The exact resolution is committed as
`scripts/fixtures/ts-baseline-package-lock.json` (174064 bytes, lockfileVersion
3), with SHA-256
`4ffbb116aa56fb46c9af791ff166486142cf884271c99a767ff145429afa2539`.
It was produced with npm `11.11.0` from the baseline `package.json` above and
is the dependency tree used for the complete checked-in oracle verification.
The prepare script verifies the source commit, `package.json`, source-tree and
snapshot hashes, copies this lock into the disposable checkout, and runs fixed
npm `11.11.0` via `npm ci --ignore-scripts --legacy-peer-deps`. Installation may
download the lock's tarballs, but it never performs a fresh semver resolution.
`--ignore-scripts` prevents dependency lifecycle scripts from changing the
baseline, and the identical source hash proves no TypeScript business source
was modified.

All strings, IDs, JSON and event fields are compared exactly. The pipeline
trace permits only a 10 ms absolute tolerance for wall-clock fields: the TS
oracle deliberately advances its fixed `Date.now()` by 1 ms per call, while
Rust injects a stable fixed `Clock`. No floating-point or LLM tolerance is used.

## Real gateway and seed replay

`scripts/differential-gateway.sh` starts the pinned TS gateway and the compiled
Rust gateway against separate temporary roots and one deterministic local
OpenAI-compatible mock. The successful seed case compares all stable HTTP
response fields, the timestamped output-path shape, exact normalized LLM
messages, isolated SQLite L0 rows, and live-store isolation.
The request includes a valid nested pipeline override plus an invalid typed
value; both implementations apply the valid field, fall back for the invalid
field, and leave the live gateway configuration unchanged.

Only `duration_ms` is excluded because it is measured wall-clock execution
time. Generated L0 IDs are normalized before prompt comparison; no functional
count, prompt text, persisted field, status, or error is given a tolerance.
The replay found and fixed that Rust seed wrote the live store instead of a
timestamped isolated store, capture assigned a different `recorded_at` per
message instead of per turn, and seed waiting forced duplicate L1 calls.
