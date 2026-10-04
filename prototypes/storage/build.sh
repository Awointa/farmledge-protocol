#!/usr/bin/env bash
# Rebuilds the THROWAWAY storage prototype: the receipt contract at BASE,
# plus src/proto.rs and its test, in a temporary directory outside the
# workspace. Runs the prototype's test, then builds the Wasm into ./out.
#
#   prototypes/storage/build.sh          # test + build
#   prototypes/storage/build.sh deploy   # also deploy to Testnet (prints the id)
#
# Never deploy this as Farmledge and never record it in deployments/.
set -euo pipefail

BASE=0f5a3a39e3b44b7aa2125cc4b18cc9be6f4d2d6d   # contract code the figures were measured on
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(git -C "$HERE" rev-parse --show-toplevel)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

git -C "$ROOT" archive "$BASE" Cargo.toml Cargo.lock rust-toolchain.toml contracts | tar -x -C "$WORK"
C="$WORK/contracts/receipt"
cp "$HERE/src/proto.rs" "$C/src/proto.rs"
cp "$HERE/src/proto_probe.rs" "$C/src/test/proto_probe.rs"
sed -i 's/^name = "farmledge-receipt"/name = "farmledge-storage-proto"/' "$C/Cargo.toml"
sed -i 's/^mod lending;/mod lending;\nmod proto;/' "$C/src/lib.rs"
sed -i 's/^mod layout;/mod layout;\nmod proto_probe;/' "$C/src/test/mod.rs"

(cd "$WORK" && cargo test -q proto_probe -- --nocapture)
(cd "$WORK" && stellar contract build --out-dir "$HERE/out" >&2)
echo "built $HERE/out/farmledge_storage_proto.wasm" >&2

if [ "${1:-}" = deploy ]; then
  ADMIN=$(stellar keys address farmledge-admin)
  stellar contract deploy --wasm "$HERE/out/farmledge_storage_proto.wasm" \
    --source-account farmledge-admin --network testnet -- --admin "$ADMIN" | tail -1
fi
