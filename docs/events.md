# Receipt contract events

This file is the contract between the protocol and the backend indexer. If
it disagrees with `contracts/receipt/src/events.rs`, the code is right and
this file is a bug. Change both in the same commit.

## Indexer requirements: a correctness requirement, not an operational nicety

Since `data_version` 2 the chain stores only each receipt's **current**
weight. A partial exit, lock or unlock reduces the receipt in place, and the
only record of what it weighed before is the `split` event. RPC providers
keep events for about 7 days. After that, a missed event can only be
recovered by replaying ledger history from Stellar's history archives
(Galexie or captive core), which is slow and costly: treat it as disaster
recovery, not a fallback. **If the indexer loses a `split` event, the
history of that receipt's weight is lost.** Finished (`Exited`, `Revoked`)
receipts are also left to archive on-chain, so the event archive is their
practical record too.

The indexer must therefore meet all four of these. Failing any of them is a
data-loss bug, to be treated with the same priority as a contract bug.

1. **Archive every event durably, `split` above all.** Write each event of
   this contract (raw XDR plus ledger sequence, transaction hash and event
   index) to durable storage **before** advancing the ingestion cursor.
   Writes are idempotent on (ledger, transaction hash, event index), so
   re-ingesting a range is always safe. Never prune the archive.
2. **Alert if indexing falls more than one day behind.** Page when the last
   ledger indexed is more than 17,280 ledgers (about 24 hours) behind the
   network's latest ledger. That leaves about 6 days of RPC retention to
   catch up before events become unrecoverable from RPC.
3. **Verify there are no ledger gaps after any outage**, of the indexer, its
   database or its RPC provider. Confirm that the indexed ledgers cover every
   ledger from the deployment's `start_ledger` (in `deployments/<network>.json`)
   to the cursor without a hole, and re-fetch any gap with `getEvents` while
   it is still inside RPC retention. Then reconcile: for each receipt touched
   in the outage window, the minted weight minus every `taken_weight_kg`
   split off it (following `parent_id`) must equal its current weight from
   `get_receipt`.
4. **Back up the archive.** Off-site, on a schedule, with restores tested.
   The backup is the second copy of data that exists nowhere else.

The same requirements are stated in `docs/archival.md` and the README.

## Encoding

Every event is a `#[contractevent]` struct:

- **Topics:** `[event_name, ...topic fields]`. `event_name` is the struct name
  in `snake_case`, as a `Symbol` (`Minted` → `minted`, `TtlExtended` →
  `ttl_extended`). The remaining topics are the fields marked **T** below, in
  the order given.
- **Data:** an `ScMap` keyed by field name (`Symbol`) holding every other
  field. Map keys are sorted, so read the data **by name, never by position**.

Types are Soroban types: `u64`/`u32` integers, `Address` (`G…` or `C…`
strkey), `Symbol`, `String`, `Option<T>` (absent = `void`), `bool`. `Status`
is a unit enum, encoded as a one-element vector holding its variant name,
e.g. `["Locked"]`. All timestamps are **ledger timestamps in Unix seconds**,
taken from the ledger that included the transaction.

### Self-containment rule

Every receipt or sale event carries `receipt_id` (or `sale_id`), `owner`,
`warehouse_id`, a quantity in kg and a timestamp. Lending events also carry
`lender`. The indexer never has to read state back to apply an event. The
registry events below carry their subject and a timestamp.

The quantity field is named `weight_kg` on events that describe the receipt
itself (`minted`, `exited`, `revoked`, `ttl_extended`) and `kg` on lending
events, where it is the pledged or sold quantity. In both cases it is the
receipt's book weight after any split in the same transaction.

### Ordering within one transaction

A partial `exit`, `lock` or `unlock` emits **`split` first**, then the
operation's own event. A split happens **in place**: the parent keeps its id
and becomes the remainder (`split.remainder_id == split.parent_id`), with its
status, owner and pledge unchanged and its weight and bag count reduced. The
operation's event names the **taken** receipt (`split.taken_id`), which is
new. Apply the events in emission order.

**Archive every `split` event.** It is the only record of the receipt's
earlier weight. See [Indexer requirements](#indexer-requirements-a-correctness-requirement-not-an-operational-nicety).

`extend_ttl_batch` emits one `ttl_extended` per receipt it keeps alive, in
the order the ids were given. `Exited` and `Revoked` receipts are skipped and
emit nothing.

## Receipt lifecycle

### `minted`: Active, newly created (`mint`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | contract-assigned, starts at 1 |
| warehouse_id | T | Symbol | |
| deposit_id | | String | client intake reference, unique per mint |
| owner | | Address | the farmer |
| commodity | | Symbol | e.g. `MAIZE_WHITE` |
| grade | | Symbol | |
| weight_kg | | u64 | book weight |
| weight_source | | WeightSource | `["Scale"]` or `["BagEstimate"]` |
| bag_count | | u32 | |
| moisture_pct | | u32 | basis points of a percent (1250 = 12.50%) |
| custodian | | Address | co-signer |
| verifier | | Address | co-signer |
| deposit_date | | u64 | timestamp |

### `split`: the parent is reduced in place and one receipt is created (inside a partial `exit` / `lock` / `unlock`)

| field | T | type | notes |
|---|---|---|---|
| parent_id | T | u64 | keeps its id, status and pledge, and becomes the remainder |
| owner | | Address | inherited by the new receipt |
| warehouse_id | | Symbol | inherited by the new receipt |
| status | | Status | the parent's status, unchanged; the new receipt starts with it too |
| lender | | Option<Address> | the parent's pledge, if it was `Locked` / `Recovery`; the new receipt starts with it too |
| parent_weight_kg | | u64 | the parent's weight **before** the split, = taken + remainder |
| taken_id | | u64 | the new receipt, which the operation acts on; its `parent_id` is `parent_id` |
| taken_weight_kg | | u64 | |
| taken_bag_count | | u32 | |
| remainder_id | | u64 | always equal to `parent_id` |
| remainder_weight_kg | | u64 | the parent's weight after the split |
| remainder_bag_count | | u32 | |
| split_at | | u64 | timestamp |

The new receipt also inherits `deposit_id`, `commodity`, `grade`,
`weight_source`, `moisture_pct`, `custodian`, `verifier`, `deposit_date`, the
lock terms and recovery history, which the indexer already holds for the
parent. Taken and remainder bag counts sum to the parent's before the split.

### `exited`: Active becomes Exited (`exit`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | the whole receipt, or the new receipt split off by a partial exit |
| warehouse_id | T | Symbol | |
| owner | | Address | |
| custodian | | Address | who signed the release |
| weight_kg | | u64 | book weight retired |
| bag_count | | u32 | |
| exit_weight_kg | | u64 | actual weight out; shrinkage = weight_kg − exit_weight_kg |
| exit_date | | u64 | timestamp |

### `revoked`: Active becomes Revoked (`revoke`, admin)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | |
| warehouse_id | T | Symbol | |
| owner | | Address | |
| weight_kg | | u64 | |
| reason | | String | stored only here, not in contract state |
| revoked_at | | u64 | timestamp |

### `ttl_extended`: no state change (`extend_ttl`, `extend_ttl_batch`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | |
| warehouse_id | T | Symbol | |
| owner | | Address | |
| status | | Status | current status |
| weight_kg | | u64 | |
| extended_at | | u64 | timestamp |

This records that a keep-alive was **requested**. The host only extends an
entry whose remaining TTL is below the threshold, so it is not proof that
rent was paid. `extend_index_ttl` emits nothing.

## Lending

### `locked`: Active becomes Locked (`lock`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | the whole receipt, or the new receipt split off by a partial lock |
| lender | T | Address | |
| owner | | Address | |
| warehouse_id | | Symbol | |
| kg | | u64 | pledged quantity |
| loan_ref | | String | lender's own reference; loan terms stay off-chain |
| due_date | | u64 | timestamp |
| grace_period | | u64 | seconds after `due_date` |
| locked_at | | u64 | timestamp |

### `unlocked`: Locked or Recovery becomes Active (`unlock`, lender)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | the released receipt (on a partial unlock, the new receipt split off) |
| lender | T | Address | |
| owner | | Address | released grain always goes to the owner |
| warehouse_id | | Symbol | |
| loan_ref | | String | |
| kg | | u64 | released quantity |
| partial | | bool | |
| remaining_receipt_id | | Option<u64> | if partial: the original receipt, still pledged with the rest |
| from_recovery | | bool | true: the open recovery is recorded as `Repaid` |
| unlocked_at | | u64 | timestamp |

### `due_extended`: Locked stays Locked (`extend_due`, lender)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | |
| lender | T | Address | |
| owner | | Address | farmer is notified (SDD §5.4) |
| warehouse_id | | Symbol | |
| loan_ref | | String | |
| kg | | u64 | |
| old_due_date | | u64 | |
| new_due_date | | u64 | strictly later |
| grace_period | | u64 | unchanged |
| extended_at | | u64 | timestamp |

### `recovery_started`: Locked becomes Recovery (`start_recovery`, lender)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | |
| lender | T | Address | |
| warehouse_id | T | Symbol | topic, so the custodian alert routes by filter |
| owner | | Address | |
| loan_ref | | String | |
| kg | | u64 | |
| due_date | | u64 | |
| grace_period | | u64 | |
| started_at | | u64 | timestamp, > due_date + grace_period |

### `recovery_completed`: Recovery becomes Active under the buyer; Sale created (`complete_recovery`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | |
| lender | T | Address | |
| sale_id | | u64 | new canonical `Sale` |
| operator | | Address | co-signer |
| previous_owner | | Address | |
| buyer | | Address | the new `owner` |
| warehouse_id | | Symbol | |
| loan_ref | | String | |
| kg | | u64 | sold quantity |
| sale_ref | | String | escrow deposit reference |
| started_at | | u64 | when recovery began |
| completed_at | | u64 | timestamp |

The receipt is re-indexed under `buyer`, and the sale under `lender`
(`get_sales_by_lender`). It also stays listed under `previous_owner` as
history.

### `settlement_recorded`: Sale goes from unsettled to settled, write-once (`record_settlement`)

| field | T | type | notes |
|---|---|---|---|
| sale_id | T | u64 | |
| lender | T | Address | |
| receipt_id | | u64 | the receipt as sold; it may since have split or exited |
| operator | | Address | |
| previous_owner | | Address | |
| buyer | | Address | |
| warehouse_id | | Symbol | |
| loan_ref | | String | |
| kg | | u64 | |
| sale_ref | | String | |
| settlement_ref | | String | bank payout reference |
| settled_at | | u64 | timestamp |

## Registries (admin)

Each event has its subject as a topic and a timestamp in the data.

| event | topics | data |
|---|---|---|
| `warehouse_added` | warehouse_id: Symbol | added_at: u64 |
| `warehouse_removed` | warehouse_id: Symbol | removed_at: u64 |
| `custodian_added` | warehouse_id: Symbol | custodian: Address, added_at: u64 |
| `custodian_removed` | warehouse_id: Symbol | custodian: Address, removed_at: u64 |
| `verifier_added` | verifier: Address | added_at: u64 |
| `verifier_removed` | verifier: Address | removed_at: u64 |
| `recovery_operator_added` | operator: Address | added_at: u64 |
| `recovery_operator_removed` | operator: Address | removed_at: u64 |

## State transitions and their events

| transition | call | events, in order |
|---|---|---|
| (none) → Active | `mint` | `minted` |
| Active → Exited | `exit` (full) | `exited` |
| Active (reduced) + new Exited receipt | `exit` (partial) | `split`, `exited` |
| Active → Revoked | `revoke` | `revoked` |
| Active → Locked | `lock` (full) | `locked` |
| Active (reduced) + new Locked receipt | `lock` (partial) | `split`, `locked` |
| Locked → Active | `unlock` (full) | `unlocked` (`from_recovery = false`) |
| Locked (reduced) + new Active receipt | `unlock` (partial) | `split`, `unlocked` (`partial = true`) |
| Locked → Locked | `extend_due` | `due_extended` |
| Locked → Recovery | `start_recovery` | `recovery_started` |
| Recovery → Active | `unlock` (full) | `unlocked` (`from_recovery = true`) |
| Recovery (reduced) + new Active receipt | `unlock` (partial) | `split`, `unlocked` (`from_recovery = true`) |
| Recovery → Active (new owner); Sale created | `complete_recovery` | `recovery_completed` |
| Sale unsettled → settled | `record_settlement` | `settlement_recorded` |
| (none) | `extend_ttl`, `extend_ttl_batch` | `ttl_extended` × n, none for `Exited` / `Revoked` |
| (none) | `extend_index_ttl` | (none) |
| registry changes | `add_*` / `remove_*` | the matching registry event |

Deploying the contract runs its constructor, which emits nothing. The
indexer should treat the deploy ledger as the start of history (see the
README, "After a Testnet reset").

## Retention

Stellar RPC keeps events for about 7 days. The indexer has to archive every
event it ingests (SDD §7). If it falls behind by more than the retention
window, rebuild from contract state: `get_receipts_by_owner`,
`get_receipts_by_warehouse`, `get_receipts_by_lender`, `get_sales_by_lender`
and `get_unsettled_sales`, all paged at 64.

## Planned for `data_version` 3: anchors and farmer proofs (not implemented)

Everything in this section is **design, not code**. It describes the A+B
storage strategy (`docs/storage-strategy.md`) and becomes binding when that
ships. Until then the rule at the top of this file still holds: the code is
right.

### `anchored` (planned): a warehouse's day is committed

| field | T | type | notes |
|---|---|---|---|
| warehouse_id | T | Symbol | |
| day | | u32 | days since 1970-01-01 UTC; strictly increasing per warehouse, never after the ledger's day |
| root | | BytesN<32> | Merkle root of the day's leaves |
| head | | BytesN<32> | `sha256(previous head ‖ day as 4 bytes, big-endian ‖ root)`; the first head uses 32 zero bytes as the previous one |
| leaves | | u32 | leaf count, which fixes the proof depth |

The indexer requirements above apply in full: `anchored` events are the only
permanent record of past roots.

### Farmer proofs

Every deposit and dispatch delivers a **proof bundle** to the farmer,
automatically (see `docs/backend-requirements.md`). A bundle proves one
leaf: one deposit, one dispatch, or one lot's balance at the end of one day.

**Bundle contents:**

| field | |
|---|---|
| `v` | bundle format version, 1 |
| `network` | network passphrase |
| `contract` | the receipt contract's id |
| `leaf` | every field of the leaf, as specified in `docs/storage-strategy.md` (B: leaf format), including `salt` |
| `proof` | the sibling hashes from the leaf up to the root, in order |

The encoding (binary: the leaf's XDR followed by the raw proof hashes, then
Base45 for an alphanumeric QR code) comes to about 1 KB for a warehouse of
2,000 lots, which fits a QR code printed at about 4 cm. The exact encoding
is fixed at implementation, and must decode with open tooling.

### Verifying a proof with nothing but the bundle and the chain

Anyone can do this with a public Stellar RPC endpoint and no access to
Farmledge's API, database or website:

1. **Check the contract.** It must be the published Farmledge contract,
   with the audited Wasm hash. This is the one fact taken from outside the
   bundle, and it is public.
2. **Hash the leaf:** `sha256(0x00 ‖ XDR(leaf))`, where the XDR is the
   leaf encoded as a Soroban `ScVal`. Any Stellar XDR library does this.
   Alternatively, simulate the contract's `leaf_hash`, a free read that
   needs no signature.
3. **Fold the proof:** start from the leaf hash. For each sibling, if the
   current position (`leaf.index`, halved at each level) is even, hash
   `0x01 ‖ current ‖ sibling`, otherwise `0x01 ‖ sibling ‖ current`. The
   result is the claimed root.
4. **Compare with the chain:**
   - **The leaf's day is within the last 30 days:** read the stored day root
     for `(warehouse_id, day)` with `getLedgerEntries` and compare. Or
     simulate the contract's `verify_leaf(leaf, proof)`. One call, free.
   - **Older:** get the `anchored` events for that warehouse from the
     leaf's day onwards from ledger history. Use `getEvents` within RPC
     retention; beyond that, any full-history source (Stellar's public
     history archives, or a public ledger data lake). Check that the
     leaf's day carries the claimed root. Recompute the head forward through
     every later day's root, and compare the result with the head the
     contract stores now. The events need not be trusted: a wrong one cannot
     reproduce the stored head.

**What a valid proof shows:** this exact leaf (lot, owner, weight, grade,
attesters) was in that warehouse's committed books for that day, and the
books for that day cannot have been changed since. **What it does not
show:** that the books were complete, or that the grain is physically
there. Those rest on audits and the warehouse.

A balance proof older than the latest anchor says what the lot held *then*.
To check current holdings, ask the farmer for their latest monthly
statement, which carries a balance proof less than 30 days old.
