#!/usr/bin/env bash
# Build, deploy and (optionally) bootstrap the receipt contract.
#
#   scripts/deploy.sh                       # testnet, admin = farmledge-admin
#   NETWORK=testnet ADMIN=G... scripts/deploy.sh
#   REGISTRY=deployments/testnet.registry scripts/deploy.sh
#
# stdout carries exactly one line: the deployment record as JSON. Everything
# else goes to stderr, so the backend can run
#
#   CONTRACT_ID=$(scripts/deploy.sh | jq -r .contract_id)
#
# The same record is written to deployments/$NETWORK.json, along with
# deployments/$NETWORK.env for shells and make.
#
# Environment:
#   NETWORK    stellar-cli network name             (default: testnet)
#   SOURCE     stellar-cli identity that deploys    (default: farmledge-admin)
#   ADMIN      contract admin address               (default: SOURCE's address)
#   REGISTRY   optional bootstrap file, one entry per line:
#                warehouse  <WAREHOUSE_ID>
#                custodian  <WAREHOUSE_ID> <G...>
#                verifier   <G...>
#                operator   <G...>
#              Registry calls are signed by SOURCE, so they only work when
#              SOURCE is the admin (the Testnet default).
set -euo pipefail

NETWORK=${NETWORK:-testnet}
SOURCE=${SOURCE:-farmledge-admin}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WASM=$ROOT/target/wasm32v1-none/release/farmledge_receipt.wasm
OUT_DIR=$ROOT/deployments

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v stellar >/dev/null || die "stellar-cli not found"
command -v jq >/dev/null || die "jq not found"

log "building"
(cd "$ROOT" && stellar contract build >&2)
[ -f "$WASM" ] || die "build produced no $WASM"

# After a Testnet reset the local key still exists but its account does not,
# so fund every time on a test network. Friendbot refuses an account that is
# already funded; that is fine.
if ! stellar keys address "$SOURCE" >/dev/null 2>&1; then
  log "creating identity $SOURCE"
  stellar keys generate "$SOURCE" --network "$NETWORK" >&2
fi
if [ "$NETWORK" = testnet ] || [ "$NETWORK" = futurenet ] || [ "$NETWORK" = local ]; then
  log "funding $SOURCE"
  stellar keys fund "$SOURCE" --network "$NETWORK" >&2 2>&1 || true
fi
DEPLOYER=$(stellar keys address "$SOURCE")
ADMIN=${ADMIN:-$DEPLOYER}

# Indexer start: a ledger at or before the deploy. Nothing of this contract
# can exist earlier.
START_LEDGER=$(stellar ledger latest --network "$NETWORK" --output json | jq -r .sequence)
PASSPHRASE=$(stellar network ls -l | awk -v n="$NETWORK" '
  $1=="Name:" {cur=$2} cur==n && /^Network passphrase:/ {sub(/^Network passphrase: /,""); print; exit}')
RPC_URL=$(stellar network ls -l | awk -v n="$NETWORK" '
  $1=="Name:" {cur=$2} cur==n && /^RPC url:/ {print $3; exit}')

# The constructor installs the admin inside the deploy transaction itself, so
# there is no window in which the contract exists without one.
log "deploying to $NETWORK as $SOURCE, admin $ADMIN"
CONTRACT_ID=$(stellar contract deploy \
  --wasm "$WASM" \
  --source-account "$SOURCE" \
  --network "$NETWORK" \
  -- --admin "$ADMIN" | tail -1)
[[ "$CONTRACT_ID" =~ ^C[A-Z2-7]{55}$ ]] || die "deploy returned '$CONTRACT_ID'"
log "contract $CONTRACT_ID"

invoke() {
  stellar contract invoke --id "$CONTRACT_ID" --source-account "$SOURCE" \
    --network "$NETWORK" -- "$@" >&2
}

if [ -n "${REGISTRY:-}" ]; then
  [ -f "$REGISTRY" ] || die "REGISTRY file $REGISTRY not found"
  log "bootstrapping registries from $REGISTRY"
  while read -r kind a b; do
    case "$kind" in
      ''|'#'*) continue ;;
      warehouse) invoke add_warehouse --warehouse_id "$a" ;;
      custodian) invoke add_custodian --warehouse_id "$a" --custodian "$b" ;;
      verifier)  invoke add_verifier --verifier "$a" ;;
      operator)  invoke add_recovery_operator --operator "$a" ;;
      *) die "unknown registry entry '$kind'" ;;
    esac
  done < "$REGISTRY"
fi

DATA_VERSION=$(stellar contract invoke --id "$CONTRACT_ID" --source-account "$SOURCE" \
  --network "$NETWORK" --send=no -- data_version)
WASM_HASH=$(sha256sum "$WASM" | cut -d' ' -f1)
GIT_COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)
if ! git -C "$ROOT" diff --quiet HEAD -- contracts 2>/dev/null; then
  GIT_COMMIT="$GIT_COMMIT-dirty"
fi

mkdir -p "$OUT_DIR"
RECORD=$(jq -nc \
  --arg network "$NETWORK" \
  --arg passphrase "$PASSPHRASE" \
  --arg rpc_url "$RPC_URL" \
  --arg contract_id "$CONTRACT_ID" \
  --arg wasm_hash "$WASM_HASH" \
  --arg admin "$ADMIN" \
  --arg deployer "$DEPLOYER" \
  --argjson start_ledger "$START_LEDGER" \
  --argjson data_version "$DATA_VERSION" \
  --arg git_commit "$GIT_COMMIT" \
  --arg deployed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{network:$network, network_passphrase:$passphrase, rpc_url:$rpc_url,
    contract_id:$contract_id, wasm_hash:$wasm_hash, admin:$admin,
    deployer:$deployer, start_ledger:$start_ledger,
    data_version:$data_version, git_commit:$git_commit,
    deployed_at:$deployed_at}')

printf '%s\n' "$RECORD" | jq . > "$OUT_DIR/$NETWORK.json"
printf '%s\n' "$RECORD" | jq -r 'to_entries[] | "\(.key|ascii_upcase)=\(.value|tostring|@sh)"' \
  > "$OUT_DIR/$NETWORK.env"
log "recorded in deployments/$NETWORK.json and deployments/$NETWORK.env"

printf '%s\n' "$RECORD"
