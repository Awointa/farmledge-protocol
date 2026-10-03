# farmledge-protocol

The Soroban smart contract behind Farmledge: a digital warehouse receipt for
maize deposited at a partner warehouse, used as loan collateral and, on
default, sold through bank escrow. See the Farmledge System Design Document
v2.3 (§1.2, §4, §5) for the full design — this repo implements a single
contract, `receipt`, with commodity as a field rather than a separate
contract per commodity.

## Current state

The deposit-to-exit lifecycle (`mint`, `exit`, `revoke`, the registries),
the lending lifecycle (`lock`, `unlock`, `extend_due`, `start_recovery`,
`complete_recovery`, `record_settlement`) and the rent keep-alive calls
(`extend_ttl`, `extend_ttl_batch`, `extend_index_ttl`) are implemented,
tested and deployed to Testnet.

The contract is **immutable**: nothing can replace its Wasm, and changes ship
as a new deployment. See [docs/upgrade.md](docs/upgrade.md).

| Document | What it is |
|---|---|
| [docs/events.md](docs/events.md) | Every event, field and type, and the transition that emits it. **The contract between protocol and backend.** |
| [docs/archival.md](docs/archival.md) | Storage rent, the keep-alive job, the restore path, and measured Testnet costs |
| [docs/upgrade.md](docs/upgrade.md) | Why the contract is immutable, and the migration path |
| [docs/adversarial-review.md](docs/adversarial-review.md) | Every way a farmer or lender could lose what they are owed, and the test that prevents it |

Deliberately **not** on-chain: user-to-user transfers, interest rates, fees,
pricing and settlement. Lenders keep loan terms off-chain and reference them
through `loan_ref`.

## Deployments

The current deployment is recorded, machine-readably, in
[`deployments/testnet.json`](deployments/testnet.json). That file is the
source of truth for the backend.

| Network | Contract ID | Notes |
|---|---|---|
| Testnet | `CBTFO2N4A7ILUX6H72FFZOGWF62ZCAKB4I6X6CI52Q2522DVNKECW6KU` | **Current.** Immutable build with a constructor, `data_version` 1, batched keep-alive, `get_sales_by_lender`, and self-contained `split` / `ttl_extended` / registry events. Deployed 2026-10-01 |
| Testnet | `CCHVG565ABRRCZCVBZLO3FM42EILZXQMT62HKRT7DMGJUATOJSVQZ7RB` | Superseded 2026-10-01. Lending lifecycle with sale records |
| Testnet | `CAZJUMEN2JMUT7HUNRUXC6ABSX5M2UOZ3RIBJPRYATHIA7L2QOUHKTET` | Superseded 2026-09-30. First lending build |

Each deployment is fresh; no state is carried over between them.

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
| `extend_ttl`, `extend_ttl_batch`, `extend_index_ttl` | nobody | at most `MAX_TTL_BATCH` (25) receipts or pages per call |
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

One per transition, and self-contained: each one carries `receipt_id` (or
`sale_id`), owner, warehouse, quantity, a timestamp and, for lending, the
lender. The indexer never has to read state back. The full field-by-field
reference, including topic order and encoding, is
**[docs/events.md](docs/events.md)**.

## Reads

Every list read is paged, 64 per page, to stay under the per-transaction
ledger-read limit, and each has a page count:

| Read | Page count |
|---|---|
| `get_receipts_by_owner(owner, page)` | `owner_page_count(owner)` |
| `get_receipts_by_warehouse(warehouse_id, page)` | `warehouse_page_count(warehouse_id)` |
| `get_receipts_by_lender(lender, page)` | `lender_page_count(lender)` |
| `get_sales_by_lender(lender, page)` | `lender_sale_page_count(lender)` |
| `get_unsettled_sales(page)` | `unsettled_sale_count()` (a count of sales, not pages) |

Single records: `get_receipt`, `get_sale`, `get_lock`, `is_pledgeable`,
`data_version`. Indexes are append-only audit
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
| stellar-cli | 28.1.0 in CI (27.0.0 also works) | `cargo install --locked stellar-cli --version 28.1.0` |
| jq, python3 | any recent | for `scripts/` |

## Development

```bash
make test            # unit + property tests
make ci              # fmt-check, clippy -D warnings, immutability check, test, build
make deploy-testnet  # build + deploy + record deployments/testnet.json
make contract-id     # print the recorded contract id
```

The admin is set by the contract's constructor, inside the deploy
transaction, so there is no moment when the contract exists without an
owner. In production the admin must be a Stellar multisig account (design
doc §6). The contract only calls `require_auth()` on it.

## Deploying, and after a Testnet reset

Stellar Testnet is reset periodically. **Everything is lost**: the contract,
all receipts, and every account, including the admin's and every farmer's,
custodian's, lender's and operator's. Redeploying is one command:

```bash
scripts/deploy.sh                                           # or: make deploy-testnet
REGISTRY=deployments/testnet.registry scripts/deploy.sh     # also re-register roles (see .registry.example)
```

It builds, funds the deployer through Friendbot, deploys, optionally registers
warehouses, custodians, verifiers and recovery operators from a `REGISTRY`
file (format in the script header), and writes `deployments/testnet.json`.
**stdout is exactly one JSON line** and nothing else:

```json
{"network":"testnet","network_passphrase":"Test SDF Network ; September 2015",
 "rpc_url":"https://soroban-testnet.stellar.org/","contract_id":"C…",
 "wasm_hash":"…","admin":"G…","deployer":"G…","start_ledger":4971770,
 "data_version":1,"git_commit":"…","deployed_at":"2026-10-01T19:20:54Z"}
```

so `CONTRACT_ID=$(scripts/deploy.sh | jq -r .contract_id)` works.
`deployments/testnet.env` holds the same fields as shell-quoted `KEY=value`
lines.

### What the backend must do after a reset

1. **Detect it.** The contract instance no longer exists:
   `stellar contract info interface --id $CONTRACT_ID --network testnet`
   fails, and `getLedgerEntries` on the instance key returns nothing. The
   latest ledger sequence also drops far below the recorded `start_ledger`.
   Stop the writers (mint, lock and exit jobs) as soon as either check fires.
2. **Redeploy:** run `scripts/deploy.sh` with the registry file, or have
   protocol ops run it, and read the new record.
3. **Swap the contract id** in configuration: `contract_id`,
   `network_passphrase` and `rpc_url` from the record. The application's
   environment guard (SDD §11.1) should refuse to start against a contract
   id it was not configured for.
4. **Re-fund every key it manages** through Friendbot: farmers, custodians,
   verification agents, recovery buyers and the operator. A `G…` address
   can only authorise a contract call if its account exists, so every
   signature fails until this is done. Lenders who hold their own keys must
   re-fund theirs.
5. **Re-register roles** if `REGISTRY` was not used: warehouses, then
   custodians per warehouse, verifiers and recovery operators, all signed by
   the admin.
6. **Reset the indexer:** wipe the Testnet database (Testnet and live
   environments already use separate databases, SDD §11.1), set the event
   cursor to `start_ledger`, and clear the archived-event store for the old
   contract id.
7. **Reset the keep-alive job's receipt list** to empty. It refills from the
   indexer. A stale list fails loudly: `extend_ttl_batch` rejects an unknown
   id with `ReceiptNotFound` rather than skipping it.
8. **Re-mint nothing automatically.** Testnet receipts are not legal records
   (SDD §11.1), and inventing them would break the "no invented data" rule.
   If the inventory trial is running on Testnet at the time, re-capture from
   the warehouse's paper records through the normal mint flow, with fresh
   `deposit_id`s.

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
└── test/         per-area tests, adversarial cases, property tests
docs/             events, archival, upgrade, adversarial review
scripts/          deploy.sh (one-command deploy), measure_costs.py (Testnet costs)
deployments/      the current deployment record per network
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
- **Rent:** receipts need a keep-alive job calling `extend_ttl_batch`, because
  no single bump covers a receipt's 6–12 month life. Schedule, restore path
  and measured costs are in [docs/archival.md](docs/archival.md).
- **`data_version`** is stamped on every `Receipt` and `Sale`; see
  [docs/upgrade.md](docs/upgrade.md).

## Open decisions

- Custodian signing method on the warehouse tablet.
- Grace period and recovery sale window lengths (defaults live in
  `farmledge-app`'s config, not on-chain).
- Grading standard and shrinkage tolerance per commodity.
- Escrow bank partner for Phase 1.

## Licence

Apache 2.0 — see [LICENSE](./LICENSE).
