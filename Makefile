# Farmledge receipt contract.
#
#   make build           optimised wasm via stellar-cli
#   make test            unit + property tests
#   make fmt / fmt-check
#   make clippy          warnings are errors
#   make ci              everything CI runs, in CI's order
#   make deploy-testnet  build, deploy, record deployments/testnet.json
#   make contract-id     print the recorded contract id

NETWORK    ?= testnet
SOURCE     ?= farmledge-admin
# Defaults to SOURCE's own address; override for a multisig admin:
#   make deploy-testnet ADMIN=G...
ADMIN      ?=
WASM       := target/wasm32v1-none/release/farmledge_receipt.wasm
DEPLOYMENT := deployments/$(NETWORK).json
REGISTRY   ?=

.PHONY: help build test fmt fmt-check clippy ci clean deploy-testnet contract-id check-immutable

help:
	@sed -n '2,10p' $(MAKEFILE_LIST) | sed 's/^# \?//'

build:
	stellar contract build

test:
	cargo test --workspace

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

ci: fmt-check clippy check-immutable test build

# Leaves deployments/ alone: those records are committed.
clean:
	cargo clean

# --- deployment -------------------------------------------------------------
#
# One command: build, deploy (the constructor installs the admin in the same
# transaction), optionally bootstrap registries, and record the result in
# deployments/$(NETWORK).json. stdout is that JSON record and nothing else.
#
#   make deploy-testnet
#   make deploy-testnet REGISTRY=deployments/testnet.registry
#   make deploy-testnet ADMIN=G...        # a multisig admin

deploy-testnet:
	@NETWORK=$(NETWORK) SOURCE=$(SOURCE) ADMIN=$(ADMIN) REGISTRY=$(REGISTRY) scripts/deploy.sh

contract-id:
	@jq -r .contract_id $(DEPLOYMENT)

# The contract must stay immutable (docs/upgrade.md): no Wasm-replacing
# entry point may ever be added.
check-immutable:
	@! grep -rn "update_current_contract_wasm" contracts/*/src \
		|| { echo "contract must stay immutable: see docs/upgrade.md"; exit 1; }
