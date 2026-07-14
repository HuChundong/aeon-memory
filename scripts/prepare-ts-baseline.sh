#!/usr/bin/env bash
set -euo pipefail

sha="${AEON_MEMORY_TS_BASELINE_SHA:-4339e63650920871eb0e8888083a1779d114e3ae}"
url="${AEON_MEMORY_TS_BASELINE_URL:-https://github.com/TencentCloud/TencentDB-Agent-Memory.git}"
destination="${1:-$(mktemp -d "${TMPDIR:-/tmp}/aeon-memory-ts-baseline.XXXXXX")}"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock_snapshot="$repo/scripts/fixtures/ts-baseline-package-lock.json"
expected_lock="4ffbb116aa56fb46c9af791ff166486142cf884271c99a767ff145429afa2539"
expected_package="5860b90231bfd7b50ac1967e71f79d9198c593bb7a5566be5c108f24f9d52307"
expected_source="9efff8e246d52bb96353865efc66a79888b5136e950a5bbba400ad2bae3837ec"

[[ -f "$lock_snapshot" ]] || { echo "missing TS dependency lock snapshot: $lock_snapshot" >&2; exit 2; }
snapshot_hash="$(shasum -a 256 "$lock_snapshot" | cut -d' ' -f1)"
[[ "$snapshot_hash" == "$expected_lock" ]] || { echo "repository TS lock snapshot drift: $snapshot_hash" >&2; exit 3; }

if [[ -e "$destination" && ! -d "$destination/.git" && -n "$(ls -A "$destination")" ]]; then
  echo "destination is not a Git checkout: $destination" >&2
  exit 2
fi
if [[ ! -d "$destination/.git" ]]; then
  git clone --filter=blob:none --no-checkout "$url" "$destination" >&2
fi
git -C "$destination" fetch --depth=1 origin "$sha" >&2
git -C "$destination" checkout --detach "$sha" >&2
[[ "$(git -C "$destination" rev-parse HEAD)" == "$sha" ]]

# This historical revision has no usable lock. The repository snapshot is the
# dependency resolution that produced the checked-in oracles; never resolve a
# fresh tree from mutable registry metadata here.
package_hash="$(shasum -a 256 "$destination/package.json" | cut -d' ' -f1)"
[[ "$package_hash" == "$expected_package" ]] || { echo "TS baseline package.json drift: $package_hash" >&2; exit 3; }
source_before="$(cd "$destination" && find src -type f -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"
[[ "$source_before" == "$expected_source" ]] || { echo "TS baseline source drift before bootstrap: $source_before" >&2; exit 3; }
cp "$lock_snapshot" "$destination/package-lock.json"
lock_hash="$(shasum -a 256 "$destination/package-lock.json" | cut -d' ' -f1)"
[[ "$lock_hash" == "$expected_lock" ]] || { echo "TS dependency lock drift: $lock_hash" >&2; exit 3; }
npm_version="$(npx --yes npm@11.11.0 --version)"
[[ "$npm_version" == "11.11.0" ]] || { echo "unexpected npm bootstrap version: $npm_version" >&2; exit 3; }
(cd "$destination" && npx --yes npm@11.11.0 ci --ignore-scripts --no-audit --no-fund --legacy-peer-deps) >&2
lock_after="$(shasum -a 256 "$destination/package-lock.json" | cut -d' ' -f1)"
[[ "$lock_after" == "$expected_lock" ]] || { echo "npm ci changed TS dependency lock: $lock_after" >&2; exit 3; }
source_after="$(cd "$destination" && find src -type f -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"
[[ "$source_before" == "$source_after" ]] || { echo "TS business source changed during bootstrap" >&2; exit 3; }
[[ -x "$destination/node_modules/.bin/tsx" ]] || { echo "TS baseline bootstrap did not install tsx" >&2; exit 3; }
printf '%s\n' "$destination"
