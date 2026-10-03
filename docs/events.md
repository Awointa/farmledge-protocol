# Receipt contract events

This file is the contract between the protocol and the backend indexer. If
it disagrees with `contracts/receipt/src/events.rs`, the code is right and
this file is a bug. Change both in the same commit.

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

**Archive every `split` event.** Because the parent is reduced in place, the
chain stores only a receipt's current weight. Its earlier weights exist only
in these events, and RPC keeps events for about 7 days, so the indexer's
own archive is the permanent record. The same is true of `exited` and
`revoked`: finished receipts are no longer kept alive (see
`docs/archival.md`), so their state is restored only on demand.

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
