# farmledge-protocol

The Soroban smart contract behind Farmledge: a digital warehouse receipt for
maize deposited at a partner warehouse, used as loan collateral and, on
default, sold through bank escrow. See the Farmledge System Design Document
v2.3 (§1.2, §4, §5) for the full design — this repo implements a single
contract, `receipt`, with commodity as a field rather than a separate
contract per commodity.

## Current state

Daily anchoring of each warehouse's books (`anchor`), minting on demand
from them (`mint_from_anchor`, `preclaim`), the deposit-to-exit lifecycle
(`exit`, `revoke`, the registries), the lending lifecycle (`lock`, `unlock`,
`extend_due`, `start_recovery`, `complete_recovery`, `record_settlement`)
and the rent keep-alive calls (`extend_ttl`, `extend_ttl_batch`,
`extend_index_ttl`) are implemented, tested and deployed to Testnet as
`data_version` 3. **The contract is frozen** until field use gives a reason
to change it.

The contract is **immutable**: nothing can replace its Wasm, and changes ship
as a new deployment. See [docs/upgrade.md](docs/upgrade.md).

**A deposit is not a receipt.** Each warehouse's books (deposits,
dispatches, end-of-day balances) are committed on-chain once a day as a
Merkle root, and a receipt is minted only when a lot is pledged, from its
balance leaf in the latest anchored day, with a proof. Farmers hold proofs of
their unpledged lots, checkable against the chain without Farmledge. Why,
the audit behind it and the cutover plans are in
[docs/storage-strategy.md](docs/storage-strategy.md); measured costs in
[docs/archival.md](docs/archival.md).

| Document | What it is |
|---|---|
| [docs/events.md](docs/events.md) | Every event, field and type, the transition that emits it, and the indexer's correctness requirements. **The contract between protocol and backend.** |
| [docs/archival.md](docs/archival.md) | Storage rent, the keep-alive job, the restore path, and measured costs for Testnet and mainnet |
| [docs/storage-strategy.md](docs/storage-strategy.md) | What to keep on-chain and why (A+B, implemented as `data_version` 3), the audit of every check, the freshness invariant, pre-claiming, and the Testnet and mainnet cutover plans |
| [docs/backend-requirements.md](docs/backend-requirements.md) | What the backend must do for the protocol to be correct: indexer, keep-alive, anchoring, farmer proof delivery, the mint freeze, cutover |
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
| Testnet | `CBW6TA5UJMXJY2VYPJV6WUVWNUP3PKZNPXFMC2CNVCWZDF3SKIT4UAIN` | **Current.** `data_version` 3: daily anchoring of each warehouse's books, receipts minted on demand from an anchored balance leaf (`mint_from_anchor`), the deposit-number claim bitmap and `preclaim`; `mint` and the `DepositIndex` guard removed. Deployed 2026-10-05 |
| Testnet | `CB3PY2P5ZJ5TQIUEAZRGXOYKXKAZZVAMXCRG25GRWLURTKMKNXKYFPEM` | **Retired** 2026-10-05. `data_version` 2: compact storage, splits in place, finished receipts left to archive, 64-byte caps on `deposit_id` and the lending refs. Held only test and measurement receipts |
| Testnet | `CDLSGBNISQ7F3ROSEE5MHAZEZYNUV5LOGQA2EWTP4BPODLQT4G2LZJW3` | **Retired** 2026-10-04. As current, but without the `deposit_id` cap. The `data_version` 2 costs in `docs/archival.md` were measured on it |
| Testnet | `CBTFO2N4A7ILUX6H72FFZOGWF62ZCAKB4I6X6CI52Q2522DVNKECW6KU` | **Retired** 2026-10-03. `data_version` 1: immutable build with a constructor, batched keep-alive, `get_sales_by_lender`, self-contained events |
| Testnet | `CCHVG565ABRRCZCVBZLO3FM42EILZXQMT62HKRT7DMGJUATOJSVQZ7RB` | **Retired** 2026-10-01. Lending lifecycle with sale records |
| Testnet | `CAZJUMEN2JMUT7HUNRUXC6ABSX5M2UOZ3RIBJPRYATHIA7L2QOUHKTET` | **Retired** 2026-09-30. First lending build |

Each deployment is fresh; no state is carried over between them. **Retired** deployments still exist on Testnet but are not used by the backend: send nothing to them, and index only the current one. Their records are kept as `deployments/testnet.v1.*` (`CBTFO2N4…`), `deployments/testnet.v2.*` (`CDLSGBNI…`) and `deployments/testnet.v2b.*` (`CB3PY2P5…`), marked `"status": "retired"`.

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
| (none) | Active | `mint_from_anchor`, from the lot's balance leaf in the latest anchored day |
| Active | Locked | `lock` |
| Active | Exited | `exit` |
| Active | Revoked | `revoke` |
| Locked | Active | `unlock` (full or partial) |
| Locked | Locked | `extend_due` |
| Locked | Recovery | `start_recovery` |
| Recovery | Active | `unlock` — farmer repaid before the sale completed |
| Recovery | Active | `complete_recovery` — owner becomes the buyer, who then exits or pledges |

Everything else is rejected with `InvalidStatus`. In particular a `Locked` or
`Recovery` receipt can never be exited, revoked, or split by its owner. To
void a pledged receipt (e.g. a fraudulent mint discovered mid-loan), the
lender unlocks it first and then the admin revokes it.

**Partial operations split in place.** `exit`, `lock` and `unlock` given
less than the full weight divide the receipt through one internal helper
(`split.rs`). The receipt keeps its id, status and pledge, and its weight and
bag count drop to what is left. The portion acted on becomes one new receipt
whose `parent_id` names it, and its id is returned. A locked portion and its
free remainder are then fully independent receipts.

A receipt's weight is therefore **not fixed for life**. Across all the
receipts of one deposit it always sums to what was minted. The chain keeps
only current weights, and the `split` event recording each division is the
history. **Archiving every `split` event durably is a correctness
requirement for the indexer, not an operational nicety**: see "Indexer
requirements" in [docs/events.md](docs/events.md). This halves the cost of
every partial operation compared with retiring the parent and writing two
children (`docs/archival.md`).

## Authorisation

Soroban has no implicit caller. Every signer is either an explicit
parameter that gets `require_auth()`d, or — for anyone with privileges — an
address loaded from storage. Only `lock` takes the lender as a parameter;
every later lending call authorises the lender **recorded in the lock**.

| Function | Must sign | Also required |
|---|---|---|
| `anchor` | custodian | custodian registered for the warehouse; `last anchored day < day ≤ ledger day`; 1 to 2^20 leaves |
| `mint_from_anchor` | custodian **and** verifier | custodian registered for the leaf's warehouse; verifier registered; a version-1 balance leaf from the warehouse's latest anchored day, naming no receipt, whose proof folds to that day's root at exactly the tree's depth; `(warehouse_id, deposit_no)` never claimed; weight, moisture and `deposit_id` (≤ 64 bytes) valid. Every receipt field comes from the leaf |
| `preclaim` | admin | warehouse registered; at most 25 numbers, none already claimed (all-or-nothing) |
| `exit` | custodian **and** owner | custodian registered for the receipt's warehouse; status `Active` |
| `revoke` | admin | status `Active` |
| `extend_ttl`, `extend_ttl_batch`, `extend_index_ttl` | nobody | at most `MAX_TTL_BATCH` (25) receipts or pages per call; `Exited` and `Revoked` receipts are skipped |
| `lock` | lender **and** owner | status `Active`; `weight_source == Scale`; `lender != owner`; `due_date > now`; `loan_ref` at most `MAX_REF_LEN` (64) bytes |
| `unlock` | recorded lender | status `Locked` or `Recovery` |
| `extend_due` | recorded lender | status `Locked`; new date strictly later |
| `start_recovery` | recorded lender | status `Locked`; ledger time `> due_date + grace_period` |
| `complete_recovery` | recovery operator **and** recorded lender | operator registered; status `Recovery`; `sale_ref` at most 64 bytes |
| `record_settlement` | recovery operator **and** the sale's lender | operator registered (any, not only the one who sold); sale exists; settlement not yet recorded; `settlement_ref` at most 64 bytes |
| registry add/remove | admin | — |

Lenders are not registered: any address can lend. Recovery operators are.
The buyer in `complete_recovery` does not sign — like the farmer at mint,
they are the beneficiary. Reads (`get_*`, `leaf_hash`, `verify_leaf`,
`is_claimed`, `data_version`, the page counts) need no signature.

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
  earlier record. A receipt split off another inherits it.
- A completed recovery also creates one canonical **`Sale`** record, stored
  under its `sale_id`, and the history entry points at it. The settlement is
  written to the `Sale`, not the receipt, because by the time the bank
  settles the buyer may already have split or exited the receipt — every
  receipt's history still resolves to the same, settled sale. Keying by
  `sale_id` rather than `receipt_id` also keeps two sales of the same receipt
  apart.
- `get_unsettled_sales(page)` / `unsettled_sale_count()` list completed sales
  still awaiting `record_settlement`; each drops off once settled.
- `extend_ttl(receipt_id)` also keeps alive every `Sale` the receipt's
  history refers to.

Current state lives in storage. Events are for the indexer and expire from
RPC after about a week. The indexer's event archive is the permanent history:
a receipt's earlier weights exist only in `split` events, and `Exited` and
`Revoked` receipts are no longer kept alive, so they archive. They stay
restorable, and reading one restores it.

Receipts and sales are stored in a compact positional layout
(`layout.rs`), not as named-field maps, which more than halves their size and
rent. Reads return the same `Receipt` and `Sale` types as before.

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
trails: they include exited, released and sold receipts, so filter on
`status` (and on `owner` / `lock` where it matters). A receipt sold in
recovery is listed under both the farmer and the buyer; a receipt re-pledged
to the same lender after a full unlock appears twice in that lender's index.

## Privacy

On-chain data is **pseudonymous**: Stellar addresses, weights, grades,
warehouse ids, loan and sale references. Names, phone numbers, farmer
consent records and loan terms stay off-chain in `farmledge-app` and the
lenders' own systems. Anything written here is public and permanent, so
references such as `loan_ref`, `sale_ref` and `settlement_ref` must be
opaque identifiers, never personal data. Each, and `deposit_id`, is capped
at 64 bytes (`MAX_REF_LEN`), which fits a UUID, a bank reference or a hex
SHA-256.

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
 "data_version":2,"git_commit":"…","deployed_at":"2026-10-03T23:11:50Z"}
```

so `CONTRACT_ID=$(scripts/deploy.sh | jq -r .contract_id)` works.
`deployments/testnet.env` holds the same fields as shell-quoted `KEY=value`
lines.

### What the backend must do after a reset

1. **Detect it.** The contract instance no longer exists:
   `stellar contract info interface --id $CONTRACT_ID --network testnet`
   fails, and `getLedgerEntries` on the instance key returns nothing. The
   latest ledger sequence also drops far below the recorded `start_ledger`.
   Stop the writers (anchor, mint, lock and exit jobs) as soon as either check fires.
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
   If the inventory trial is running on Testnet at the time, re-anchor the
   books from the database on the new contract (it starts a fresh chain per
   warehouse) and mint only what is pledged, through the normal flow.

## Repository layout

```
contracts/receipt/src/
├── lib.rs        contract entry points
├── anchor.rs     anchor / mint_from_anchor / preclaim / Merkle proofs
├── lending.rs    lock / unlock / extend_due / recovery / record_settlement
├── split.rs      the one place a receipt is divided
├── types.rs      Receipt, Leaf, AnchorHead, Lock, Recovery, Sale, Status, WeightSource
├── errors.rs     contract Error enum (codes are public ABI; never renumber)
├── events.rs     typed #[contractevent] structs
├── storage.rs    DataKey layout, TTL bumps, paged indexes, claim bitmap, anchors
├── admin.rs      admin and the warehouse/custodian/verifier/operator registries
└── test/         per-area tests, adversarial cases, property tests
docs/             events, archival, storage strategy, backend requirements, upgrade, adversarial review
scripts/          deploy.sh, measure_costs.py (Testnet costs), cost_model.py,
                  verify_proof.py (standalone proof check), check_events_doc.py
deployments/      the current deployment record per network
```

## Design notes

- **`Lock` / `Recovery` are `MaybeLock` / `MaybeRecovery` enums, not
  `Option`s.** `Option<CustomStruct>` as a `#[contracttype]` field fails to
  compile under `testutils` on soroban-sdk 28; see `types.rs`.
- **Time** is the ledger timestamp. `due_date` and `grace_period` are seconds
  supplied by the lender; the contract enforces ordering, not policy.
- **Moisture** is basis points of a percent (`1250` = 12.50%).
- **`receipt_id`** is contract-assigned. A lot's identity is its
  per-warehouse `deposit_no`, assigned once by the backend at intake; the
  claim bitmap keyed on `(warehouse_id, deposit_no)` makes each lot mint at
  most once, ever. `deposit_id` is the human reference.
- **Rent:** minted receipts need a keep-alive job calling `extend_ttl_batch`, because
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
