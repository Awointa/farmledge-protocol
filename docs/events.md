# Receipt contract events

This file is the contract between the protocol and the backend indexer, for
the deployment with `data_version` 3. If it disagrees with
`contracts/receipt/src/events.rs`, the code is right and this file is a bug.
Change both in the same commit; `scripts/check_events_doc.py` compares the
two and fails on any difference in events, fields, types or topics.

## Indexer requirements: a correctness requirement, not an operational nicety

The chain stores only each receipt's **current** weight. A partial exit,
lock or unlock reduces the receipt in place, and the only record of what it
weighed before is the `split` event. A day's Merkle root is stored on-chain
for 30 days only; after that **the `anchored` event is the only record of
the root**, and without it no proof for that day can be checked against the
chain except by replaying ledger history. RPC providers keep events for about
7 days. After that, a missed event can only be recovered by replaying ledger
history from Stellar's history archives (Galexie or captive core), which is
slow and costly: treat it as disaster recovery, not a fallback. Finished
(`Exited`, `Revoked`) receipts are also left to archive on-chain, so the
event archive is their practical record too.

The indexer must therefore meet all four of these. Failing any of them is a
data-loss bug, to be treated with the same priority as a contract bug.

1. **Archive every event durably, `split` and `anchored` above all.** Write
   each event of this contract (raw XDR plus ledger sequence, transaction
   hash and event index) to durable storage **before** advancing the
   ingestion cursor. Writes are idempotent on (ledger, transaction hash,
   event index), so re-ingesting a range is always safe. Never prune the
   archive.
2. **Alert if indexing falls more than one day behind.** Page when the last
   ledger indexed is more than 17,280 ledgers (about 24 hours) behind the
   network's latest ledger. That leaves about 6 days of RPC retention to
   catch up before events become unrecoverable from RPC.
3. **Verify there are no ledger gaps after any outage**, of the indexer, its
   database or its RPC provider. Confirm that the indexed ledgers cover every
   ledger from the deployment's `start_ledger` (in `deployments/<network>.json`)
   to the cursor without a hole, and re-fetch any gap with `getEvents` while
   it is still inside RPC retention. Then reconcile:
   - for each receipt touched in the outage window, the `minted` weight
     minus every `taken_weight_kg` split off it (following `parent_id`) must
     equal its current weight from `get_receipt`;
   - for each warehouse, recomputing the head from its archived `anchored`
     events (`head = sha256(prev_head ‖ day ‖ leaves ‖ root)`, each event's
     `prev_head` equal to the previous event's `head`) must give the head
     `get_anchor` returns now. A break means an `anchored` event is missing.
4. **Back up the archive.** Off-site, on a schedule, with restores tested.
   The backup is the second copy of data that exists nowhere else.

The same requirements are stated in `docs/archival.md`,
`docs/backend-requirements.md` and the README.

## Encoding

Every event is a `#[contractevent]` struct:

- **Topics:** `[event_name, ...topic fields]`. `event_name` is the struct name
  in `snake_case`, as a `Symbol` (`Minted` → `minted`, `TtlExtended` →
  `ttl_extended`). The remaining topics are the fields marked **T** below, in
  the order given.
- **Data:** an `ScMap` keyed by field name (`Symbol`) holding every other
  field. Map keys are sorted, so read the data **by name, never by position**.

Types are Soroban types: `u64`/`u32` integers, `Address` (`G…` or `C…`
strkey), `Symbol`, `String`, `BytesN<32>` (32 raw bytes, an `ScBytes`),
`Option<T>` (absent = `void`), `bool`. `Status` and `WeightSource` are unit
enums, encoded as a one-element vector holding the variant name, e.g.
`["Locked"]`, `["Scale"]`. All timestamps are **ledger timestamps in Unix
seconds**, taken from the ledger that included the transaction. A `day` is a
UTC day number: Unix seconds ÷ 86,400, rounded down.

### Self-containment rule

Every receipt or sale event carries `receipt_id` (or `sale_id`), `owner`,
`warehouse_id`, a quantity in kg and a timestamp. Lending events also carry
`lender`. The indexer never has to read state back to apply an event. The
anchoring and registry events carry their subject and a timestamp.

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
emit nothing. `preclaim` emits one `preclaimed` per number, in the order
given.

## Anchoring and minting

### `anchored`: a warehouse's day is committed (`anchor`)

| field | T | type | notes |
|---|---|---|---|
| warehouse_id | T | Symbol | |
| day | | u32 | UTC day; strictly after the warehouse's previous anchored day, never after the ledger's day |
| leaves | | u32 | leaf count of the day's tree, 1 to 2^20; fixes the proof depth, `ceil(log2(leaves))` |
| root | | BytesN<32> | Merkle root of the day's leaves |
| prev_head | | BytesN<32> | the head before this anchor; 32 zero bytes on the warehouse's first |
| head | | BytesN<32> | `sha256(prev_head ‖ day ‖ leaves ‖ root)`, `day` and `leaves` as 4-byte big-endian |
| custodian | | Address | who signed the anchor |
| anchored_at | | u64 | timestamp |

The root is stored on-chain (temporary storage) for 30 days, then
deleted by the network. **This event is then the only record of it.**

### `minted`: Active, newly created (`mint_from_anchor`)

| field | T | type | notes |
|---|---|---|---|
| receipt_id | T | u64 | contract-assigned, starts at 1 |
| warehouse_id | T | Symbol | the leaf's |
| deposit_no | | u64 | the lot's identity; each `(warehouse_id, deposit_no)` mints once, ever |
| deposit_id | | String | the lot's intake reference, at most 64 bytes |
| owner | | Address | the farmer |
| commodity | | Symbol | e.g. `MAIZE_WHITE` |
| grade | | Symbol | |
| weight_kg | | u64 | book weight: the leaf's `kg` |
| weight_source | | WeightSource | `["Scale"]` or `["BagEstimate"]` |
| bag_count | | u32 | |
| moisture_pct | | u32 | basis points of a percent (1250 = 12.50%) |
| custodian | | Address | attested the **intake** (from the leaf) |
| verifier | | Address | countersigned the **intake** (from the leaf) |
| deposit_date | | u64 | when the lot was taken in (the leaf's `deposited_at`), not the mint time |
| minted_by_custodian | | Address | signed the mint |
| minted_by_verifier | | Address | signed the mint |
| anchor_day | | u32 | the day of the balance leaf minted from: always the warehouse's latest anchored day at the time |
| leaf_hash | | BytesN<32> | that leaf's hash, so the receipt can be checked against the anchored books |
| minted_at | | u64 | timestamp |

Every field from `deposit_no` to `deposit_date` is copied from the proven
leaf; nothing in the receipt comes from the caller.

### `preclaimed`: a deposit number can never be minted (`preclaim`, admin)

| field | T | type | notes |
|---|---|---|---|
| warehouse_id | T | Symbol | |
| deposit_no | | u64 | claimed without a mint |
| claimed_at | | u64 | timestamp |

Only for a cutover from an older contract holding real lots, where these
events are the published, checkable pre-claim list
(`docs/storage-strategy.md`, "Migration path"). **No such cutover is to be
built** — Testnet abandoned v2 and mainnet launches on v3 — so on the
current deployments the indexer should expect none, and alert on any.

## Receipt lifecycle

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

The new receipt also inherits `deposit_id`, `deposit_no`, `commodity`, `grade`,
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
| warehouse's day committed | `anchor` | `anchored` |
| (none) → Active | `mint_from_anchor` | `minted` |
| deposit numbers claimed, no receipt | `preclaim` | `preclaimed` × n |
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
| warehouse registered / removed | `add_warehouse` / `remove_warehouse` | `warehouse_added` / `warehouse_removed` |
| custodian registered / removed | `add_custodian` / `remove_custodian` | `custodian_added` / `custodian_removed` |
| verifier registered / removed | `add_verifier` / `remove_verifier` | `verifier_added` / `verifier_removed` |
| recovery operator registered / removed | `add_recovery_operator` / `remove_recovery_operator` | `recovery_operator_added` / `recovery_operator_removed` |

A call that fails emits nothing and changes nothing. Deploying the contract
runs its constructor, which emits nothing. The indexer should treat the
deploy ledger as the start of history (see the README, "After a Testnet
reset").

## Retention

Stellar RPC keeps events for about 7 days. The indexer has to archive every
event it ingests (SDD §7). If it falls behind by more than the retention
window, rebuild receipts from contract state: `get_receipts_by_owner`,
`get_receipts_by_warehouse`, `get_receipts_by_lender`, `get_sales_by_lender`
and `get_unsettled_sales`, all paged at 64. **Anchors cannot be rebuilt from
state:** the contract keeps only each warehouse's latest head
(`get_anchor`) and the last 30 days' roots. Missing `anchored` events must
be recovered from ledger history; the head tells you when the recovered set
is complete.

## Farmer proofs

Every deposit and dispatch delivers a **proof bundle** to the farmer,
automatically (see `docs/backend-requirements.md`). A bundle proves one leaf:
one deposit, one dispatch, or one lot's balance at the end of one day.

**Verification never goes through Farmledge.** A bundle is checked against
the chain, through any public Stellar RPC endpoint, with any tool that can
compute SHA-256 and encode Stellar XDR. The script below needs nothing else:
no Farmledge code, API, database or website. A verification page hosted by
Farmledge is only a convenience; nothing about a proof depends on it, and a
lender or a farmer's representative should run an independent check.

### Bundle format

A bundle is one JSON object:

| field | type | |
|---|---|---|
| `rpc` | string | a Stellar RPC endpoint; any public one will do, the verifier may override it |
| `contract_id` | string | the receipt contract (`C…`). Check it against the published deployment, and its Wasm hash against the audited build: this is the one fact taken from outside the bundle |
| `leaf` | object | every field of the leaf, below, including its `salt` |
| `proof` | array of hex strings | sibling hashes from the leaf up to the root, in order |
| `day_leaves` | number | the leaf count of the leaf's day (from its `anchored` event) |
| `prev_head` | hex string | `prev_head` of the leaf's day's `anchored` event |
| `later_days` | array | for every later anchored day of the warehouse, oldest first: `{"day", "leaves", "root"}` from its `anchored` event. Empty when the bundle is fresh |

The leaf, with the JSON type each field takes:

| field | Soroban type | JSON | |
|---|---|---|---|
| version | u32 | number | 1 |
| kind | u32 | number | 0 deposit, 1 dispatch, 2 end-of-day balance |
| warehouse_id | Symbol | string | |
| day | u32 | number | UTC day of the tree |
| index | u32 | number | position in the tree, **never an identity** |
| deposit_no | u64 | number | the lot's identity |
| deposit_id | String | string | the lot's intake reference |
| receipt_id | u64 | number | receipt governing the lot, or 0 |
| receipt_version | u32 | number | `data_version` of `receipt_id`'s contract, or 0 |
| owner | Address | string | |
| commodity, grade | Symbol | string | |
| kg | u64 | number | deposited, dispatched or remaining |
| weight_source | WeightSource | string | `"Scale"` or `"BagEstimate"` |
| bags, moisture_bps | u32 | number | |
| custodian, verifier | Address | string | who attested the intake |
| deposited_at | u64 | number | intake time, the same on every leaf of a lot |
| at | u64 | number | time of this leaf's event; the cutoff for a balance |
| salt | BytesN<32> | hex string | random per leaf |

The leaf is the contract's `Leaf` type; the same JSON is what
`stellar contract invoke … -- leaf_hash --leaf '<json>'` takes.

**Hashing.** leaf = `sha256(0x00 ‖ XDR(leaf as ScVal))`, where the ScVal is
a map keyed by field name, keys sorted; node = `sha256(0x01 ‖ left ‖
right)`; a tree is padded to a power of two with `sha256(0x00)`. Leaves are
ordered by (kind, deposit_no). The 0x00 / 0x01 prefixes stop a leaf being
passed off as a node.

### Verifying a bundle: standalone script

`scripts/verify_proof.py`, reproduced here in full so it can be copied out
of this document. Python 3.8+, standard library only. Run it as
`python3 verify_proof.py bundle.json --rpc https://…`. Always pass `--rpc`
with an endpoint you chose: the RPC is the one thing the check trusts, and
a forged bundle can name a server that lies.

```python
#!/usr/bin/env python3
"""Check a Farmledge proof bundle against the Stellar chain, with nothing
but Python's standard library and a public Stellar RPC.

    python3 verify_proof.py bundle.json [--rpc URL]

The last line printed is always `RESULT <CODE>`; the codes are in OUTCOMES
below. Exit 0: the leaf was in the warehouse's anchored books for its day.
The bundle format is in docs/events.md ("Farmer proofs"). This file is
copied verbatim into that document; it uses no Farmledge code or API.
"""
import base64, hashlib, json, struct, sys, urllib.request

# code: (exit status, what it means). Exit 0 is the only "verified".
OUTCOMES = {
    "OK": (0, "the root is on-chain for the leaf's day, and the head chain reaches the head on-chain"),
    "DAY_ROOT_EXPIRED": (0, "the day's root has left the chain (after 30 days, as designed); verified through the head chain alone"),
    "NOT_YET_ANCHORED": (2, "the warehouse has not anchored the leaf's day yet: not a failure, check again after the next anchor"),
    "PROOF_DOES_NOT_FIT": (1, "the proof does not fit a tree of day_leaves leaves"),
    "DAY_IN_FUTURE": (1, "the leaf's day is after the chain's current day, so it can never be anchored"),
    "DAY_ROOT_DIFFERS": (1, "the root stored on-chain for the leaf's day is not the root this proof gives"),
    "NO_HEAD": (1, "the warehouse has never anchored anything on-chain"),
    "HEAD_MISMATCH": (1, "the head chain from the leaf's day does not reach the head on-chain"),
    "BAD_BUNDLE": (4, "the bundle is not a well-formed proof bundle"),
    "COULD_NOT_CHECK": (3, "the chain could not be read: this is no result at all"),
}

class Outcome(Exception): pass

def finish(code, detail=""):
    print(f"{code}: {OUTCOMES[code][1]}" + (f" ({detail})" if detail else ""))
    print(f"RESULT {code}")
    sys.exit(OUTCOMES[code][0])

B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"

def strkey(s):  # G... account or C... contract -> (kind, 32 bytes)
    if len(s) != 56: raise ValueError(f"bad address {s}")  # 35 bytes exactly: one spelling per key
    bits = "".join(format(B32.index(c), "05b") for c in s)
    raw = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits) - 7, 8))
    body, crc = raw[:-2], int.from_bytes(raw[-2:], "little")
    c = 0
    for byte in body:  # CRC16-XModem
        c ^= byte << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x1021 if c & 0x8000 else c << 1) & 0xFFFF
    kind = {6 << 3: "account", 2 << 3: "contract"}.get(body[0])
    if c != crc or kind is None or len(body) != 33:
        raise ValueError(f"bad address {s}")
    return kind, body[1:]

# --- XDR, just the ScVal types a leaf and its keys use ---------------------
u32 = lambda n: struct.pack(">I", n)
u64 = lambda n: struct.pack(">Q", n)
opaque = lambda b: u32(len(b)) + b + b"\0" * (-len(b) % 4)

def address(s):
    kind, key = strkey(s)
    return u32(0) + u32(0) + key if kind == "account" else u32(1) + key

def sv(t, v):  # ScVal
    if t == "u32": return u32(3) + u32(v)
    if t == "u64": return u32(5) + u64(v)
    if t == "bytes": return u32(13) + opaque(v)
    if t == "string": return u32(14) + opaque(v.encode())
    if t == "symbol": return u32(15) + opaque(v.encode())
    if t == "address": return u32(18) + address(v)
    if t == "vec": return u32(16) + u32(1) + u32(len(v)) + b"".join(v)
    if t == "map":  # keys sorted, as the host requires
        items = sorted(v.items())
        return u32(17) + u32(1) + u32(len(items)) + b"".join(sv("symbol", k) + x for k, x in items)

LEAF = {"version": "u32", "kind": "u32", "warehouse_id": "symbol", "day": "u32",
        "index": "u32", "deposit_no": "u64", "deposit_id": "string", "receipt_id": "u64",
        "receipt_version": "u32", "owner": "address", "commodity": "symbol",
        "grade": "symbol", "kg": "u64", "bags": "u32", "moisture_bps": "u32",
        "custodian": "address", "verifier": "address", "deposited_at": "u64",
        "at": "u64", "salt": "bytes"}

def leaf_xdr(leaf):
    f = {k: sv(t, bytes.fromhex(leaf[k]) if t == "bytes" else leaf[k]) for k, t in LEAF.items()}
    f["weight_source"] = sv("vec", [sv("symbol", leaf["weight_source"])])
    return sv("map", f)

def sha(*parts): return hashlib.sha256(b"".join(parts)).digest()

def leaf_hash(leaf): return sha(b"\x00", leaf_xdr(leaf))

def fold(h, index, proof):
    for sib in proof:
        h = sha(b"\x01", h, sib) if index % 2 == 0 else sha(b"\x01", sib, h)
        index //= 2
    return h

def chain(prev, day, leaves, root): return sha(prev, u32(day), u32(leaves), root)

# --- reading the chain: any failure here is COULD_NOT_CHECK, never a result
def rpc_call(rpc, method, params):
    req = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1,
        "method": method, "params": params}).encode(),
        {"content-type": "application/json", "user-agent": "verify-proof/2"})
    try:
        reply = json.load(urllib.request.urlopen(req, timeout=30))
        if "error" in reply: raise ValueError(reply["error"])
        return reply["result"]
    except Exception as e:
        raise Outcome("COULD_NOT_CHECK", f"{method}: {e}")

def entry(rpc, contract, key, durability):  # -> (value ScVal bytes, ledger written), or None
    lk = u32(6) + address(contract) + key + u32(durability)
    found = rpc_call(rpc, "getLedgerEntries", {"keys": [base64.b64encode(lk).decode()]}).get("entries") or []
    if not found: return None
    # An archived entry is still returned, with its value: archived is not absent.
    data = base64.b64decode(found[0]["xdr"])
    # LedgerEntryData: type, ext, contract, key, durability, then the value.
    prefix = u32(6) + u32(0) + address(contract) + key + u32(durability)
    if not data.startswith(prefix): raise Outcome("COULD_NOT_CHECK", "unexpected entry layout")
    return data[len(prefix):], found[0].get("lastModifiedLedgerSeq")

def vec_of(val, *types):  # exactly a Vec of these: "u32", or "bytes" of 32 -> python list
    if val[:8] != u32(16) + u32(1) or struct.unpack(">I", val[8:12])[0] != len(types):
        raise Outcome("COULD_NOT_CHECK", "unexpected value type")
    out, i = [], 12
    for t in types:
        tag = struct.unpack(">I", val[i:i + 4])[0]
        if t == "u32" and tag == 3: out.append(struct.unpack(">I", val[i + 4:i + 8])[0]); i += 8
        elif t == "bytes" and tag == 13 and val[i + 4:i + 8] == u32(32) and len(val) >= i + 40:
            out.append(val[i + 8:i + 40]); i += 40
        else: raise Outcome("COULD_NOT_CHECK", f"unexpected value at byte {i}")
    if i != len(val): raise Outcome("COULD_NOT_CHECK", "unexpected bytes after the value")
    return out

def check(b, rpc):
    try:
        if not (isinstance(b, dict) and isinstance(b["leaf"], dict) and isinstance(b["proof"], list)
                and isinstance(b.get("later_days", []), list)): raise ValueError("not a bundle's shape")
        leaf, proof = b["leaf"], [bytes.fromhex(p) for p in b["proof"]]
        day, n, idx = leaf["day"], b["day_leaves"], leaf["index"]
        ints = [day, n, idx] + [leaf[k] for k, t in LEAF.items() if t in ("u32", "u64")]
        ints += [x for d in b.get("later_days", []) for x in (d["day"], d["leaves"])]
        if not all(type(x) is int for x in ints): raise ValueError("integer fields must be JSON integers")
        h = leaf_hash(leaf)
        prev = bytes.fromhex(b["prev_head"])
        later = [(d["day"], d["leaves"], bytes.fromhex(d["root"])) for d in b.get("later_days", [])]
        for d in later: chain(prev, *d)  # rejects days or counts that are not u32
        if any(len(x) != 32 for x in proof + [prev] + [d[2] for d in later]): raise ValueError("hashes are 32 bytes")
        wh = sv("symbol", leaf["warehouse_id"])
        address(b["contract_id"])
    except Exception as e:
        raise Outcome("BAD_BUNDLE", f"{type(e).__name__}: {e}")
    if not 1 <= n <= 2 ** 20 or len(proof) != (n - 1).bit_length() or not 0 <= idx < n:
        raise Outcome("PROOF_DOES_NOT_FIT", f"{len(proof)} siblings, index {idx}, {n} leaves")
    root = fold(h, idx, proof)
    print(f"leaf hash  {h.hex()}\nroot       {root.hex()}")

    # The latest anchored day is in the warehouse's head; today is the chain's.
    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "Head"), wh]), 1)
    if stored is None: raise Outcome("NO_HEAD", leaf["warehouse_id"])
    last_day, on_chain_head = vec_of(stored[0], "u32", "bytes")
    today = int(rpc_call(rpc, "getLatestLedger", {})["closeTime"]) // 86400
    print(f"head       last anchored day {last_day}; chain's day now {today}")
    if day > today: raise Outcome("DAY_IN_FUTURE", f"day {day}")
    if day > last_day: raise Outcome("NOT_YET_ANCHORED", f"day {day}, last anchored {last_day}")

    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "DayRoot"), wh, sv("u32", day)]), 0)
    if stored is not None:
        on_chain_root, leaves = vec_of(stored[0], "bytes", "u32")
        if (on_chain_root, leaves) != (root, n):
            raise Outcome("DAY_ROOT_DIFFERS", f"day {day}: on-chain {on_chain_root.hex()}, {leaves} leaves")
        print(f"day root   matches the root stored on-chain for day {day} (written in ledger {stored[1]})")

    # The head commits to every anchored day: recompute it from this day on.
    head = chain(prev, day, n, root)
    for d in later: head = chain(head, *d)
    if head != on_chain_head:
        raise Outcome("HEAD_MISMATCH", "later_days is missing the anchors after this day"
                      if not later and last_day != day else f"on-chain head {on_chain_head.hex()}")
    print(f"head       matches the head on-chain (last day {last_day})")
    print(f"{leaf['kg']} kg, lot {leaf['deposit_no']}, kind {leaf['kind']}, "
          f"in {leaf['warehouse_id']}'s books on day {day}")
    return "OK" if stored is not None else "DAY_ROOT_EXPIRED"

def main():
    try:
        b = json.load(open(sys.argv[1]))
        rpc = sys.argv[sys.argv.index("--rpc") + 1] if "--rpc" in sys.argv else b["rpc"]
    except Exception as e:
        finish("BAD_BUNDLE", f"{type(e).__name__}: {e}")
    # The RPC is the one thing trusted. A bundle could name a server that lies.
    print(f"rpc        {rpc}" + ("" if "--rpc" in sys.argv else
          "  (named by the bundle: pass --rpc with an endpoint you chose)"))
    try:
        finish(check(b, rpc))
    except Outcome as o:
        finish(*o.args)
    except Exception as e:  # anything unforeseen in what the chain returned
        finish("COULD_NOT_CHECK", f"{type(e).__name__}: {e}")

if __name__ == "__main__":
    main()
```

What it checks, in order, and the outcome it reports. The last line
printed is always `RESULT <CODE>`; only exit status 0 means verified.

1. **The bundle is well formed:** every field present and of its type,
   every address a valid strkey, every hash 32 bytes. Otherwise
   `BAD_BUNDLE`. A counter-slip leaf, with `index` still `null`, is not a
   bundle and lands here.
2. **The proof fits the day:** exactly as many siblings as a tree of
   `day_leaves` leaves is deep, and an index inside it. Otherwise
   `PROOF_DOES_NOT_FIT`.
3. **The leaf hash and the root** it folds to.
4. **The warehouse has a head** (the persistent `Head(warehouse_id)` entry,
   which holds the last anchored day). Otherwise `NO_HEAD`: nothing was ever
   anchored for it.
5. **The leaf's day can be anchored.** A day after the chain's current UTC
   day (from `getLatestLedger`) never can: `DAY_IN_FUTURE`. A day after the
   head's last anchored day is `NOT_YET_ANCHORED`: the books for it have not
   been committed yet, which is not a failure. Check again after the next
   anchor. Both inputs come from the chain, not the bundle.
6. **Within 30 days of the leaf's day:** the root and leaf count stored
   on-chain for that day (the temporary `DayRoot(warehouse_id, day)` entry)
   must be the ones the proof gives. Otherwise `DAY_ROOT_DIFFERS`.
7. **Always:** recomputes the head from that day forward through
   `later_days`, and compares it with the head stored on-chain now.
   `prev_head` and `later_days` come from `anchored` events, which can be
   taken from any archive, Farmledge's included, or replayed from ledger
   history: they need not be trusted, because wrong ones cannot reproduce
   the stored head. Otherwise `HEAD_MISMATCH`.

| code | exit | meaning |
|---|---:|---|
| `OK` | 0 | verified: the root is on-chain for the day, and the head chain reaches today's head |
| `DAY_ROOT_EXPIRED` | 0 | verified through the head chain alone; the day's root left the chain after 30 days, as designed |
| `NOT_YET_ANCHORED` | 2 | not a failure: the warehouse has not anchored that day yet |
| `PROOF_DOES_NOT_FIT` | 1 | the proof cannot belong to a tree of `day_leaves` leaves |
| `DAY_IN_FUTURE` | 1 | the leaf claims a day the chain has not reached |
| `DAY_ROOT_DIFFERS` | 1 | the books committed for that day are not the ones this proof gives |
| `NO_HEAD` | 1 | the warehouse has never anchored anything |
| `HEAD_MISMATCH` | 1 | the claimed chain of days does not reach the head on-chain |
| `BAD_BUNDLE` | 4 | not a well-formed bundle |
| `COULD_NOT_CHECK` | 3 | the chain could not be read; no result at all |

An archived `Head` is still returned by RPC with its value, and is used
as is: archived is not absent.

It was run against the Testnet deployment on 2026-10-05, on a fresh bundle
and on one checked only through the head chain; see `docs/archival.md`.
The version with reason codes was run on 2026-10-12 against a synthetic
lot anchored for the purpose (warehouse `TESTNET_EXAMPLE_WH`, day 20737):
`OK` on the genuine bundle, `DAY_ROOT_DIFFERS` with its weight altered.
Every outcome is pinned by `scripts/test_verify_proof.py` against a
stand-in RPC.

**What a valid proof shows:** this exact leaf (lot, owner, weight, grade,
attesters) was in that warehouse's committed books for that day, and the
books for that day cannot have been changed since. **What it does not
show:** that the books were complete, or that the grain is physically
there. Those rest on audits and the warehouse.

A balance proof older than the latest anchor says what the lot held *then*.
To check current holdings, ask the farmer for their latest monthly
statement, which carries a balance proof less than 30 days old.

The contract's own reads give the same answers for a recent day:
`leaf_hash(leaf)` and `verify_leaf(leaf, proof)` are free to simulate. They
are a convenience too; the script does not use them.
