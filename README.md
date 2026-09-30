# farmledge-protocol

The Soroban smart contract behind Farmledge: a digital warehouse receipt for
maize deposited at a partner warehouse, used as loan collateral and, on
default, sold through bank escrow. See the Farmledge System Design Document
v2.2 (§1.2, §4, §5) for the full design — this repo implements a single
contract, `receipt`, with commodity as a field rather than a separate
contract per commodity.

## Current state

The deposit-to-exit lifecycle (`mint`, `exit`, `revoke`, `extend_ttl`, the
registries) and the lending lifecycle (`lock`, `unlock`, `extend_due`,
`start_recovery`, `complete_recovery`, `record_settlement`) are implemented,
tested and deployed to Testnet.

Deliberately **not** on-chain: user-to-user transfers, interest rates, fees,
pricing and settlement. Lenders keep loan terms off-chain and reference them
through `loan_ref`.

## Deployments

| Network | Contract ID | Notes |
|---|---|---|
| Testnet | `CCHVG565ABRRCZCVBZLO3FM42EILZXQMT62HKRT7DMGJUATOJSVQZ7RB` | **Current.** Lending lifecycle with sale records, `record_settlement` and self-contained events. Deployed 2026-09-30, admin `GBCF56H2FW3JOTAJFNPX6MROJSXSLNGMID3EXXAMDE6BANFOVR3XQKLR`, wasm sha256 `2c74231f…8bb6a6` |
| Testnet | `CAZJUMEN2JMUT7HUNRUXC6ABSX5M2UOZ3RIBJPRYATHIA7L2QOUHKTET` | Superseded 2026-09-30. First lending build (settlement recorded at sale time) |
| Testnet | _not recorded in this repo — TODO_ | Superseded. Mint/exit/revoke only |

Each deployment is fresh; no state is carried over between them.

`make deploy-testnet` writes the latest deployment to
`deployments/testnet.env`.

## Receipt state machine

```
             lock                       start_recovery
  Active ─────────────► Locked ────────────────────► Recovery
    ▲ ▲ ◄───────────────  │ ▲                            │ │
    │ │     unlock        └─┘ extend_due                 │ │
    │ │                                                  │ │
    │ └──────────────── unlock (farmer repaid) ──────────┘ │
    └────────────────── complete_recovery (owner := buyer) ┘

  Active ── exit ──► Exited      Active ── revoke ──► Revoked
```

| From | To | Via |
|---|---|---|
| Active | Locked | `lock` |
| Active | Exited | `exit` |
| Active | Revoked | `revoke` |
| Locked | Active | `unlock` (full or partial) |
| Locked | Locked | `extend_due` |
| Locked | Recovery | `start_recovery` |
| Recovery | Active | `unlock` — farmer repaid before the sale completed |
| Recovery | Active | `complete_recovery` — owner becomes the buyer, who then exits or pledges |
| Active / Locked / Recovery | Split | internal only, as part of a partial `exit`, `lock` or `unlock` |

Everything else is rejected with `InvalidStatus`. In particular a `Locked` or
`Recovery` receipt can never be exited, revoked, or split by its owner. To
void a pledged receipt (e.g. a fraudulent mint discovered mid-loan), the
lender unlocks it first and then the admin revokes it.

**Partial operations split.** `exit`, `lock` and `unlock` given less than the
full weight divide the receipt through one internal helper (`split.rs`): the
parent becomes `Split` and is never written again, and two children whose
weights and bag counts sum exactly to it take its place. The portion acted on
is the first child and its id is returned; the second keeps the parent's
status and pledge. A locked portion and its free remainder are then fully
independent receipts.

## Authorisation

Soroban has no implicit caller. Every signer is either an explicit
parameter that gets `require_auth()`d, or — for anyone with privileges — an
address loaded from storage. Only `lock` takes the lender as a parameter;
every later lending call authorises the lender **recorded in the lock**.

| Function | Must sign | Also required |
|---|---|---|
| `mint` | custodian **and** verifier | custodian registered for the warehouse; verifier registered |
| `exit` | custodian **and** owner | custodian registered for the receipt's warehouse; status `Active` |
| `revoke` | admin | status `Active` |
| `extend_ttl` | nobody | — |
| `lock` | lender **and** owner | status `Active`; `weight_source == Scale`; `lender != owner`; `due_date > now` |
| `unlock` | recorded lender | status `Locked` or `Recovery` |
| `extend_due` | recorded lender | status `Locked`; new date strictly later |
| `start_recovery` | recorded lender | status `Locked`; ledger time `> due_date + grace_period` |
| `complete_recovery` | recovery operator **and** recorded lender | operator registered; status `Recovery` |
| `record_settlement` | recovery operator **and** the sale's lender | operator registered (any, not only the one who sold); sale exists; settlement not yet recorded |
| registry add/remove | admin | — |

Lenders are not registered: any address can lend. Recovery operators are.
The buyer in `complete_recovery` does not sign — like the farmer at mint,
they are the beneficiary.

The rule that only scale-weighed receipts can be pledged lives in one
function, `lending::require_scale_weight`.

## Recovery records and settlement

A recovery sale has two references, arriving at different times (design doc
§5.5):

1. **`sale_ref`** — the escrow deposit proving the buyer paid. Recorded by
   `complete_recovery(receipt_id, operator, buyer, sale_ref)`, which returns a
   new `sale_id`.
2. **`settlement_ref`** — the bank paying the lender and any surplus to the
   farmer. Recorded later by `record_settlement(sale_id, operator,
   settlement_ref)`. Write-once.

Where each lives:

- `Receipt::recovery` holds the **open** recovery while the receipt is in
  `Recovery`.
- When it resolves — `Repaid` (lender unlocked) or `Completed` (sold) — it is
  appended to `Receipt::recovery_history`. That list is append-only: a buyer
  can pledge the receipt again and a later recovery never overwrites the
  earlier record. Split children inherit it.
- A completed recovery also creates one canonical **`Sale`** record, stored
  under its `sale_id`, and the history entry points at it. The settlement is
  written to the `Sale`, not the receipt, because by the time the bank
  settles the buyer may already have split or exited the receipt — every
  child's history still resolves to the same, settled sale. Keying by
  `sale_id` rather than `receipt_id` also keeps two sales of the same receipt
  apart.
- `get_unsettled_sales(page)` / `unsettled_sale_count()` list completed sales
  still awaiting `record_settlement`; each drops off once settled.
- `extend_ttl(receipt_id)` also keeps alive every `Sale` the receipt's
  history refers to.

Audit state lives in storage. Events are for the indexer and expire from RPC
after about a week.

## Events

One per transition, and **self-contained**: each carries everything the
indexer needs, so it never has to read a receipt back. Topics are listed
first.

| Event | Topics | Payload |
|---|---|---|
| `minted` | receipt_id, warehouse_id | deposit_id, owner, commodity, grade, weight_kg, weight_source, bag_count, moisture_pct, custodian, verifier, deposit_date |
| `split` | parent_id | owner, warehouse_id, status, taken_id / weight_kg / bag_count, remainder_id / weight_kg / bag_count |
| `exited` | receipt_id, warehouse_id | owner, custodian, weight_kg, bag_count, exit_weight_kg, exit_date |
| `revoked` | receipt_id, warehouse_id | owner, weight_kg, reason, revoked_at |
| `ttl_extended` | receipt_id | — |
| `locked` | receipt_id, lender | owner, warehouse_id, kg, loan_ref, due_date, grace_period, locked_at |
| `unlocked` | receipt_id, lender | owner, warehouse_id, loan_ref, kg, partial, remaining_receipt_id, from_recovery, unlocked_at |
| `due_extended` | receipt_id, lender | owner, warehouse_id, loan_ref, kg, old_due_date, new_due_date, grace_period, extended_at |
| `recovery_started` | receipt_id, lender, warehouse_id | owner, loan_ref, kg, due_date, grace_period, started_at |
| `recovery_completed` | receipt_id, lender | sale_id, operator, previous_owner, buyer, warehouse_id, loan_ref, kg, sale_ref, started_at, completed_at |
| `settlement_recorded` | sale_id, lender | receipt_id, operator, previous_owner, buyer, warehouse_id, loan_ref, kg, sale_ref, settlement_ref, settled_at |

A partial exit, lock or unlock is preceded, in the same transaction, by a
`split` event naming both new ids and their bag counts. Registry events carry
just their subject.

## Reads

Owner, warehouse and lender listings are paged, 64 receipts per page, to stay
under the per-transaction ledger-read limit. Indexes are append-only audit
trails: they include split, exited, released and sold receipts, so filter on
`status` (and on `owner` / `lock` where it matters). A receipt sold in
recovery is listed under both the farmer and the buyer; a receipt re-pledged
to the same lender after a full unlock appears twice in that lender's index.

## Privacy

On-chain data is **pseudonymous**: Stellar addresses, weights, grades,
warehouse ids, loan and sale references. Names, phone numbers, farmer
consent records and loan terms stay off-chain in `farmledge-app` and the
lenders' own systems. Anything written here is public and permanent, so
references such as `loan_ref`, `sale_ref` and `settlement_ref` must be
opaque identifiers, never personal data.

## Prerequisites

| Tool | Version used here | Install |
|---|---|---|
| Rust | pinned in `rust-toolchain.toml` | [rustup.rs](https://rustup.rs) |
| `wasm32v1-none` target | — | `rustup target add wasm32v1-none` |
| stellar-cli | 28.1.0 in CI | `cargo install --locked stellar-cli --version 28.1.0` |

## Development

```bash
make test            # unit + property tests
make ci              # fmt-check, clippy -D warnings, test, build — what CI runs
make deploy-testnet  # build, deploy, initialize, record in deployments/testnet.env
```

`initialize` is a separate call from deploy, so the contract is unowned
between the two; `make deploy-testnet` runs them back to back. In production
the admin must be a Stellar multisig account (design doc §6) — the contract
only calls `require_auth()` on it.

## Repository layout

```
contracts/receipt/src/
├── lib.rs        contract entry points
├── lending.rs    lock / unlock / extend_due / recovery / record_settlement
├── split.rs      the one place a receipt is divided
├── types.rs      Receipt, Lock, Recovery, Sale, Status, WeightSource
├── errors.rs     contract Error enum (codes are public ABI; never renumber)
├── events.rs     typed #[contractevent] structs
├── storage.rs    DataKey layout, TTL bumps, paged indexes
├── admin.rs      admin and the warehouse/custodian/verifier/operator registries
└── test/         per-area tests, adversarial auth cases, property tests
```

## Design notes

- **`Lock` / `Recovery` are `MaybeLock` / `MaybeRecovery` enums, not
  `Option`s.** `Option<CustomStruct>` as a `#[contracttype]` field fails to
  compile under `testutils` on soroban-sdk 28; see `types.rs`.
- **Time** is the ledger timestamp. `due_date` and `grace_period` are seconds
  supplied by the lender; the contract enforces ordering, not policy.
- **Moisture** is basis points of a percent (`1250` = 12.50%).
- **`receipt_id`** is contract-assigned and distinct from the client's
  `deposit_id`, which guards against duplicate mints.
- **Rent:** receipts need a keep-alive job calling `extend_ttl`; no single
  bump covers a receipt's 6–12 month life.

## Open decisions

- Custodian signing method on the warehouse tablet.
- Grace period and recovery sale window lengths (defaults live in
  `farmledge-app`'s config, not on-chain).
- Grading standard and shrinkage tolerance per commodity.
- Escrow bank partner for Phase 1.

## Licence

Apache 2.0 — see [LICENSE](./LICENSE).
