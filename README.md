# farmledge-protocol

The Soroban smart contract behind Farmledge: a digital warehouse receipt for
maize deposited at a partner warehouse, used as loan collateral and, on
default, sold through bank escrow. See the Farmledge System Design Document
v2.2 (§4, §7) for the full design — this repo implements a single contract,
`receipt`, with commodity as a field rather than a separate contract per
commodity.

## Current state

This is a **skeleton**: types, storage layout, auth checks, and function
signatures are final for Phase 1, but most function bodies are `todo!()`.
`mint()` and `get_receipt()` are fully implemented and tested — everything
else panics if called. Run `cargo test -- --ignored` to see the full list of
placeholder tests waiting on their implementations.

## Prerequisites

| Tool | Version used here | Install |
|---|---|---|
| Rust | 1.97.1 (pinned in `rust-toolchain.toml`) | [rustup.rs](https://rustup.rs) |
| `wasm32v1-none` target | — | `rustup target add wasm32v1-none` |
| stellar-cli | 28.0.0+ (needs ≥25.2.0 for soroban-sdk 28) | `cargo install --locked stellar-cli --version 28.0.0` |

## Setup

```bash
git clone <repo-url> farmledge-protocol
cd farmledge-protocol
rustup target add wasm32v1-none   # if not already installed
cargo test                        # runs the real tests + ignored placeholders
stellar contract build            # produces target/wasm32v1-none/release/farmledge_receipt.wasm
```

To also run the ignored placeholder tests (expected to fail with `todo!()`
until implemented):

```bash
cargo test -- --ignored
```

## Deploying to Testnet

```bash
stellar keys generate admin --network testnet --fund
stellar contract deploy \
  --wasm target/wasm32v1-none/release/farmledge_receipt.wasm \
  --source admin \
  --network testnet \
  -- --admin <ADMIN_MULTISIG_ADDRESS>
```

The `--admin` argument is passed through to `initialize()`. In production,
`ADMIN_MULTISIG_ADDRESS` must be a 2-of-3 Stellar multisig account (design
doc §6) — the contract only checks `require_auth()` on that address and does
not implement multisig logic itself.

## Repository layout

```
contracts/receipt/src/
├── lib.rs        contract entry points (mint, lock, unlock, exit, ...)
├── types.rs      Receipt, Lock, Recovery, Status, WeightSource
├── errors.rs     contract Error enum
├── events.rs     typed #[contractevent] structs, one per state change
├── storage.rs    DataKey layout, TTL bump helpers
├── admin.rs      custodian/verifier/recovery-operator registries
└── test.rs       real tests (mint, auth) + TODO-marked placeholders
```

## Design notes worth knowing before extending this

- **Lock/Recovery are not `Option` fields on `Receipt`.** soroban-sdk's
  spec-shaking codegen can't derive an infallible `ScVal` conversion for
  `Option<CustomStruct>` under `cargo test` (a known SDK limitation as of
  28.0.0). They're stored as separate persistent entries
  (`DataKey::Lock(receipt_id)`, `DataKey::Recovery(receipt_id)`) instead.
- **Every function that needs a caller's signature takes that caller's
  `Address` as an explicit parameter.** Soroban contracts have no implicit
  "msg.sender" — `complete_recovery`'s `operator` and `exit`'s `custodian`
  exist for this reason, not because the design doc listed them.
- **Moisture is stored as basis points of a percent** (`moisture_pct: u32`,
  e.g. `1250` = 12.50%) since Soroban has no floating point.
- **`receipt_id`** is a contract-assigned auto-incrementing `u64`, distinct
  from the client-generated `deposit_id` used for duplicate-mint protection.

## Open decisions

These are called out with `[bracketed notes]` in the design doc and are not
yet settled:

- Custodian signing method on the warehouse tablet.
- Grace period and recovery sale window lengths (defaults live in
  `farmledge-app`'s config, not on-chain).
- Grading standard and shrinkage tolerance per commodity.
- Escrow bank partner for Phase 1.

## Licence

Apache 2.0 — see [LICENSE](./LICENSE).
