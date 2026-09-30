# Farmledge receipt contract.
#
#   make build           optimised wasm via stellar-cli
#   make test            unit + property tests
#   make fmt / fmt-check
#   make clippy          warnings are errors
#   make ci              everything CI runs, in CI's order
#   make deploy-testnet  build, deploy, initialize, record the contract id

NETWORK    ?= testnet
SOURCE     ?= farmledge-admin
# Defaults to SOURCE's own address; override for a multisig admin:
#   make deploy-testnet ADMIN=G...
ADMIN      ?=
WASM       := target/wasm32v1-none/release/farmledge_receipt.wasm
DEPLOYMENT := deployments/$(NETWORK).env

.PHONY: help build test fmt fmt-check clippy ci clean deploy-testnet contract-id

help:
	@sed -n '2,9p' $(MAKEFILE_LIST) | sed 's/^# \?//'

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

ci: fmt-check clippy test build

clean:
	cargo clean
	rm -rf deployments

# --- deployment -------------------------------------------------------------
#
# `initialize` is a plain function rather than `__constructor`, so deploying
# and initialising are two steps. Between them the contract is unowned and
# anyone could claim it — run them back to back, which is what this target
# does, and confirm the admin afterwards.

deploy-testnet: build
	@mkdir -p deployments
	@stellar keys address $(SOURCE) >/dev/null 2>&1 \
		|| stellar keys generate $(SOURCE) --network $(NETWORK) --fund
	@admin="$(ADMIN)"; \
	if [ -z "$$admin" ]; then admin=$$(stellar keys address $(SOURCE)); fi; \
	echo "==> deploying to $(NETWORK) as $(SOURCE), admin $$admin"; \
	id=$$(stellar contract deploy \
		--wasm $(WASM) \
		--source-account $(SOURCE) \
		--network $(NETWORK) \
		2>/dev/null | tail -1); \
	echo "==> contract $$id"; \
	stellar contract invoke \
		--id $$id \
		--source-account $(SOURCE) \
		--network $(NETWORK) \
		-- initialize --admin $$admin; \
	printf 'CONTRACT_ID=%s\nNETWORK=%s\nADMIN=%s\nWASM_SHA256=%s\n' \
		"$$id" "$(NETWORK)" "$$admin" "$$(sha256sum $(WASM) | cut -d' ' -f1)" \
		> $(DEPLOYMENT); \
	echo "==> recorded in $(DEPLOYMENT)"; \
	cat $(DEPLOYMENT)

contract-id:
	@grep '^CONTRACT_ID=' $(DEPLOYMENT) | cut -d= -f2
