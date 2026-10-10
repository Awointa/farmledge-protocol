# State archival and rent

Receipts live 6–12 months. Soroban storage is rented: every persistent entry
has a TTL, and when it runs out the entry is **archived**. Archived entries
are not deleted. Since protocol 23 they are restorable at any time. But until
someone pays to restore one, a transaction that touches it pays for the
restore, and an RPC read has to go through the restore path. This document
covers the TTL policy, the keep-alive job the backend must run, the restore
path, what each operation costs (measured on Testnet, converted to mainnet),
the renewal cadence, and the monthly and per-deposit cost.

**Since `data_version` 3 a deposit is not a receipt.** Each warehouse's
books are anchored daily, and a receipt is minted only when a lot is
pledged (`docs/storage-strategy.md`). The current figures are in
[`data_version` 3: measured](#data_version-3-measured). The sections after
it measure `data_version` 1 and 2, where **every deposit minted its own
receipt that paid rent for 9–12 months**; they are kept as the record of
how the design got here, and their per-deposit figures do not apply to v3.

## `data_version` 3: measured

Deployment `CBW6TA5U…KIT4UAIN`, measured 2026-10-05 with
`scripts/measure_costs.py` (every transaction submitted for real, fees read
back from RPC), on the second run against the contract so that the farmer's
and lender's index pages already exist, as they do in steady state. Raw
numbers in `deployments/testnet.costs.json` and `testnet.entries.json`;
the tables below are `scripts/cost_model.py`'s output.

Testnet charged 5,238/KB during the run (snapshot 5147/KB, state 3.28 GB); mainnet 1000/KB on 2026-10-05 (state 1.90 GB). Rent formula check, 25-receipt extend: 4,105,147.0 predicted vs 4,105,198 charged.

Each anchor is a 2,048-leaf day (a warehouse with about 2,000 active lots),
so every mint carries an 11-level proof. All figures are XLM unless marked.

| operation | Testnet | mainnet today | mainnet at 10,000/KB | of which rent (Testnet, stroops) |
|---|---:|---:|---:|---:|
| anchor — first day (creates the head), 2,048 leaves | 0.2263 | 0.0444 | 0.4307 | 2,248,507 |
| anchor — next day, 2,048 leaves | 0.0206 | 0.0051 | 0.0380 | 191,523 |
| mint_from_anchor — first in its claim chunk (creates chunk, owner index), depth 11 | 1.6110 | 0.3122 | 3.0703 | 16,052,577 |
| mint_from_anchor — later lot, depth 11 | 0.5975 | 0.1187 | 1.1355 | 5,917,972 |
| preclaim — 25 numbers, creates their chunk | 0.3243 | 0.0646 | 0.6161 | 3,209,477 |
| preclaim — 25 numbers, chunk exists | 0.0033 | 0.0033 | 0.0033 | 0 |
| lock — full | 0.1379 | 0.0282 | 0.2611 | 1,355,567 |
| lock — partial (splits) | 0.7416 | 0.1452 | 1.4117 | 7,371,102 |
| exit — full | 0.0220 | 0.0056 | 0.0404 | 203,089 |
| exit — partial (splits) | 0.6257 | 0.1226 | 1.1911 | 6,218,621 |
| extend_ttl_batch — 2 Exited receipts (skipped) | 0.0005 | 0.0005 | 0.0005 | 0 |
| extend_due | 0.0009 | 0.0009 | 0.0009 | 0 |
| unlock — partial (splits) | 0.6204 | 0.1215 | 1.1809 | 6,165,843 |
| extend_ttl — 1 receipt, TTL above threshold (no-op bump) | 0.0006 | 0.0006 | 0.0006 | 0 |
| extend_ttl_batch — 25 receipts, TTL above threshold | 0.0053 | 0.0053 | 0.0053 | 0 |
| extend_index_ttl — 2 pages | 0.0005 | 0.0005 | 0.0005 | 0 |
| native extend — 1 receipt, +1 day (to 176 days) | 0.0037 | 0.0009 | 0.0069 | 34,934 |
| native extend — 1 receipt, +5 days (to 180-day ceiling) | 0.0167 | 0.0034 | 0.0316 | 164,430 |
| native extend — 25 receipts, to 180-day ceiling | 0.4116 | 0.0795 | 0.7849 | 4,105,198 |
| start_recovery | 0.1915 | 0.0373 | 0.3648 | 1,906,271 |
| complete_recovery | 0.5514 | 0.1086 | 1.0490 | 5,473,327 |
| record_settlement | 0.0274 | 0.0069 | 0.0505 | 253,490 |
| unlock — full | 0.0009 | 0.0009 | 0.0009 | 0 |

What stands out:

- **A mint costs 0.119 XLM on mainnet today, 18% less than v2's** (0.145),
  although it now checks an 11-level Merkle proof: the per-deposit guard
  is gone, and the proof check costs CPU, not rent (3.4M instructions in
  all, against 2.4M for v2's mint).
- **A day's anchor costs 0.005 XLM on mainnet today** (0.04 at 10,000/KB):
  the head is rewritten in place, and the root sits in temporary storage
  for 30 days at half the persistent rate.
- **Everything after the mint costs what it did in v2.** Lock, unlock,
  exit, recovery and settlement are the same code over a receipt 16 bytes
  larger (`deposit_no`).
- **A pre-claim of 25 numbers costs 0.0033 XLM** when their claim chunk
  exists, and 0.065 when it creates the chunk.

Stored entry sizes, read back with `getLedgerEntries`:

| entry | bytes |
|---|---:|
| Head | 164 |
| DayRoot (temporary) | 176 |
| Receipt — Active, freshly minted | 448 |
| Claimed chunk (1,024 deposit numbers) | 256 |
| Receipt — Locked | 544 |
| Receipt — Active, reduced in place by a split | 448 |
| OwnerIndex page 0 | 800 |
| Receipt — after a completed recovery | 668 |
| Sale — settled | 420 |

The receipt grew from 432 to 448 bytes for `deposit_no`; the 124-byte
`DepositIndex` per deposit is gone, replaced by 256 bytes per 1,024
deposit numbers.

### Per deposit and per month

**Assumptions,** the same as in `docs/storage-strategy.md`: a lot is in
store for 270 days; 30% of lots are pledged, on day 30, for 90 days, and
only those are minted; the receipt is kept alive from the mint to the exit
under the decided keep-alive policy; about 50 deposits per warehouse per
month, so each warehouse's daily anchors are shared by about 600 deposits
a year. Recoveries are rare and left out.

| | Testnet | mainnet today | mainnet at 10,000/KB |
|---|---:|---:|---:|
| anchors, per warehouse per year | 7.9481 | 1.9398 | 14.6994 |
| a pledged deposit (mint, lock, unlock, exit, keep-alive) | 1.2346 | 0.2447 | 2.3469 |
| an unpledged deposit (its share of anchors and of a claim chunk) | 0.0136 | 0.0033 | 0.0251 |
| **per deposit, 30% pledged** | 0.3839 | 0.0767 | 0.7292 |

| rate | XLM per deposit | 200 deposits / month | 2,000 deposits / month |
|---|---:|---:|---:|
| Testnet | 0.3839 | 76.8 | 768 |
| mainnet today | 0.0767 | 15.3 | 153 |
| mainnet at 10,000/KB | 0.7292 | 145.8 | 1,458 |

Against `data_version` 2 (every deposit minted and kept alive, 0.2734 XLM
per deposit on mainnet today, 2.6669 at 10,000/KB, 1.4343 on Testnet) that
is **72–73% less** at every rate: at 2,000 deposits a month, 547 → 153 XLM
a month today and 5,334 → 1,458 at 10,000/KB. The prototype's estimate for
A+B was 0.0751 and 0.7194; the built contract comes within about 2%.

**What moves the figure:** almost all of it is the pledged share. Each
point of pledge rate adds about 0.0024 XLM per deposit today (0.023 at
10,000/KB). Anchors are per warehouse: 1.94 XLM per warehouse per year on
mainnet today, 14.70 at 10,000/KB, so a warehouse with fewer deposits than
50 a month pays more per deposit for them. An idle warehouse is anchored
too, every day, with a one-leaf sentinel day: a chain that starts at the
first intake can't tell a new warehouse from one operating unrecorded, so
every registered warehouse is anchored daily from its registration day
(farmledge-app D53). Leaf count doesn't change the fee, since only the root
goes on-chain, so a sentinel day costs what a 2,048-leaf day costs (below).

| creating one claim chunk | 0.3209 | 0.0613 | 0.6127 |

| pre-claim, one call | Testnet | mainnet today | mainnet at 10,000/KB |
|---|---:|---:|---:|
| preclaim — 25 numbers, creates their chunk | 0.3243 | 0.0646 | 0.6161 |
| preclaim — 25 numbers, chunk exists | 0.0033 | 0.0033 | 0.0033 |


The raw run, with the resources each transaction declared:

| operation | fee charged (stroops) | XLM | of which rent | other resource fee | CPU instructions | disk read B | write B | footprint RO / RW | events B | tx |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| anchor — first day (creates the head), 2,048 leaves | 2,262,770 | 0.2263 | 2,248,507 | 14,163 | 1,119,951 | 0 | 340 | 4 / 2 | 408 | `7a075d921734` |
| anchor — next day, 2,048 leaves | 205,800 | 0.0206 | 191,523 | 14,177 | 1,139,694 | 0 | 340 | 4 / 2 | 408 | `f4802e95c4c0` |
| anchor — first day, 1-leaf sentinel (live, farmledge-app Testnet session) | 2,770,388 | 0.2770 | 2,750,200 | 20,088 | 1,642,247 | 144 | 408 | 5 / 3 | 404 | `07c85d92829d` |
| anchor — first day, 2 leaves (live, same session) | 2,770,387 | 0.2770 | 2,750,200 | 20,087 | 1,640,408 | 144 | 408 | 5 / 3 | 404 | `776b52f31bf7` |
| mint_from_anchor — first in its claim chunk (creates chunk, owner index), depth 11 | 16,109,509 | 1.6110 | 16,052,577 | 56,832 | 3,254,638 | 144 | 1,628 | 7 / 8 | 884 | `30769ce02eb7` |
| mint_from_anchor — later lot, depth 11 | 5,975,076 | 0.5975 | 5,917,972 | 57,004 | 3,398,673 | 144 | 1,712 | 7 / 8 | 884 | `8fb2640ac532` |
| preclaim — 25 numbers, creates their chunk | 3,242,920 | 0.3243 | 3,209,477 | 33,343 | 4,072,760 | 0 | 256 | 3 / 1 | 4,300 | `f1ac3ba4a741` |
| preclaim — 25 numbers, chunk exists | 33,459 | 0.0033 | 0 | 33,359 | 4,095,453 | 0 | 256 | 3 / 1 | 4,300 | `0240b5869e73` |
| lock — full | 1,378,733 | 0.1379 | 1,355,567 | 23,066 | 1,782,114 | 144 | 960 | 3 / 4 | 388 | `4e605662e295` |
| lock — partial (splits) | 7,416,093 | 0.7416 | 7,371,102 | 44,891 | 2,778,722 | 144 | 2,432 | 2 / 10 | 896 | `e35caec2c0b5` |
| exit — full | 219,818 | 0.0220 | 203,089 | 16,629 | 1,553,515 | 144 | 540 | 4 / 2 | 368 | `64a5b0cb7a3b` |
| exit — partial (splits) | 6,257,179 | 0.6257 | 6,218,621 | 38,458 | 2,541,646 | 144 | 2,024 | 3 / 8 | 876 | `b087a442fc74` |
| extend_ttl_batch — 2 Exited receipts (skipped) | 4,708 | 0.0005 | 0 | 4,608 | 963,321 | 0 | 0 | 5 / 0 | 0 | `6460429bb636` |
| extend_due | 9,364 | 0.0009 | 0 | 9,264 | 960,696 | 0 | 552 | 2 / 1 | 428 | `a733ccb72cde` |
| unlock — partial (splits) | 6,203,931 | 0.6204 | 6,165,843 | 37,988 | 2,238,098 | 0 | 2,424 | 1 / 9 | 992 | `025336be9555` |
| extend_ttl — 1 receipt, TTL above threshold (no-op bump) | 5,577 | 0.0006 | 0 | 5,477 | 921,553 | 0 | 0 | 4 / 0 | 288 | `a6ebd3ceef00` |
| extend_ttl_batch — 25 receipts, TTL above threshold | 52,720 | 0.0053 | 0 | 52,620 | 5,575,487 | 0 | 0 | 28 / 0 | 7,200 | `42884b2118e6` |
| extend_index_ttl — 2 pages | 5,445 | 0.0005 | 0 | 5,345 | 893,760 | 0 | 0 | 5 / 0 | 0 | `0979af73a22b` |
| native extend — 1 receipt, +1 day (to 176 days) | 37,428 | 0.0037 | 34,934 | 2,394 | 0 | 0 | 0 | 1 / 0 | 0 | `f8d741f1877f` |
| native extend — 1 receipt, +5 days (to 180-day ceiling) | 166,924 | 0.0167 | 164,430 | 2,394 | 0 | 0 | 0 | 1 / 0 | 0 | `08e6e74520cc` |
| native extend — 25 receipts, to 180-day ceiling | 4,116,482 | 0.4116 | 4,105,198 | 11,184 | 0 | 0 | 0 | 25 / 0 | 0 | `e1b6f6471751` |
| start_recovery | 1,915,436 | 0.1915 | 1,906,271 | 9,065 | 962,422 | 0 | 696 | 2 / 1 | 376 | `1306d88d5760` |
| complete_recovery | 5,514,099 | 0.5514 | 5,473,327 | 40,672 | 2,588,553 | 144 | 2,172 | 3 / 9 | 560 | `51da3a0cd670` |
| record_settlement | 274,142 | 0.0274 | 253,490 | 20,552 | 1,648,559 | 144 | 600 | 4 / 3 | 572 | `e4a1c5978ce4` |
| unlock — full | 9,068 | 0.0009 | 0 | 8,968 | 955,083 | 0 | 456 | 2 / 1 | 384 | `5e56b8bf9f19` |

The two live rows were measured on 10 Oct 2026 (contract `CBW6TA5U…`,
ledgers 5,121,645 and 5,121,650). One leaf and two leaves cost the same, to
within a stroop. They aren't directly comparable with the run's first-day row:
that contract's anchor reads one more entry and writes one more (footprint
5 / 3 against 4 / 2), and Testnet's rent rate drifts, so most of the
difference is rent. The recurring cost of an idle warehouse is a next-day
sentinel anchor, not yet measured live.

## The TTL policy

Defined in `contracts/receipt/src/storage.rs`:

| constant | ledgers | ≈ days | meaning |
|---|---:|---:|---|
| `ENTRY_TTL_THRESHOLD` | 518,400 | 30 | bump an entry only once it has less than this left… |
| `ENTRY_TTL_EXTEND_TO` | 3,024,000 | 175 | …back up to this |
| `INSTANCE_TTL_THRESHOLD` / `_EXTEND_TO` | same | 30 / 175 | the instance (admin, id counters) and the contract code |
| network `max_entry_ttl` (Testnet and mainnet, protocol 29) | 3,110,400 | 180 | the ceiling. The largest accepted bump is one ledger less. No single bump can cover a 6–12-month receipt. |
| network `min_persistent_ttl`, Testnet | 120,960 | 7 | what a newly restored entry gets |
| network `min_persistent_ttl`, mainnet | 2,073,600 | 120 | the same on mainnet |

Days assume 5-second ledgers (17,280 per day). The 5-day gap between 175
and the 180-day ceiling is headroom, so a validator vote that lowers the
ceiling slightly doesn't make every write fail.

A bump is **floor-only and idempotent**. Calling `extend_ttl` on an entry
with more than 30 days left changes nothing and charges no rent, only the
transaction's base resource fee (measured below).

### What keeps what alive

| entry | refreshed by |
|---|---|
| `Receipt(id)` | every write to that receipt (lock, unlock, exit, split, extend_due, recovery), and `extend_ttl` / `extend_ttl_batch` **unless it is `Exited` or `Revoked`**. Those are left to archive. |
| `Claimed(warehouse_id, chunk)`: the duplicate guard, one bit per deposit number, 1,024 per entry | written by a mint or pre-claim that sets a bit in it, and nothing else. It is **never kept alive**: once archived it still blocks a duplicate mint, because the mint reads it and that restores it (`ttl::an_archived_claim_chunk_still_blocks_a_duplicate_mint`). It must never be temporary storage (audit finding A3, pinned by `audit::a3_the_claim_bitmap_is_persistent_storage`). |
| `Head(warehouse_id)`: the anchor chain's head | every `anchor` of that warehouse, daily. If anchoring stops for 175 days it archives, and the next anchor or read restores it. |
| `DayRoot(warehouse_id, day)`: a day's root and leaf count | **temporary storage, 30 days, then deleted** by the network, by design. Minting only ever uses the latest day; older days are proved through the head and the `anchored` events (`docs/events.md`). |
| `Sale(id)` | complete_recovery, record_settlement, and `extend_ttl(_batch)` on any live receipt whose history names it. If every such receipt has finished before the bank settles, `record_settlement` restores the sale, at the caller's cost. |
| `UnsettledSales` | complete_recovery, record_settlement, every `extend_ttl(_batch)` |
| owner / warehouse / lender / lender-sale **index pages** and their page counters | appending to the page while it is filling. **A full page (64 ids) is never written again.** It is kept alive only by `extend_index_ttl`. |
| registry entries (warehouse, custodian, verifier, operator) | being added, and every time they are checked (mint, exit, complete_recovery, record_settlement). An idle warehouse's registration can archive. That is harmless: the next mint restores it automatically. |
| contract instance + code | every state-changing call and every keep-alive call |

`DayRoot` is the only temporary entry. Its expiry loses nothing that is not
also in the `anchored` events and committed to by the head; everything else
is persistent and can never be lost through expiry.

## The keep-alive job (backend)

SDD §7 already calls for "a daily job [that] extends the storage TTL of every
active receipt". Concretely:

**Run it daily.** An entry written at time *t* expires at *t* + 175 days. A
bump only has an effect after *t* + 145 days, when less than 30 days remain.
A daily job therefore gets about 30 attempts before anything expires, and
the job can be down for up to **29 days** without data being archived. Alert
if any tracked entry has less than **14 days** left. That means the job
has been failing for two weeks.

**Each run:**

1. **Receipts.** Take every receipt the indexer has in `Active`, `Locked` or
   `Recovery`. **Never send `Exited` or `Revoked` ids.** Nothing can act on
   them again, so they are left to archive, and the indexer's event archive
   is their permanent record. The contract skips them anyway, but once one
   has archived, naming it makes the transaction restore it, paying the
   network's minimum TTL in rent (120 days on mainnet), only for it to be
   skipped.
2. **Read their TTLs** with RPC `getLedgerEntries` (up to 200 keys per
   request), or `stellar contract read --key-xdr …`, whose last column is the
   `live_until_ledger`. Keep only those with
   `live_until_ledger − latest_ledger < 518,400`. In steady state that is
   about 1/145 of live receipts per day, because each receipt comes due
   once every 145 days.
3. **Submit `extend_ttl_batch`** in chunks of at most `MAX_TTL_BATCH` = 25
   ids, from any funded account. No signature from anyone else is needed.
   The call is all-or-nothing: an id the contract doesn't know fails the
   whole batch with `ReceiptNotFound`. Treat that as "the receipt list has
   drifted from the chain" (after a Testnet reset, say) and fix the list. Do
   not drop the id and retry.
4. **Index pages.** For each owner, warehouse and lender with
   `*_page_count > 0`, and each lender with `lender_sale_page_count > 0`,
   read the pages' TTLs the same way and call
   `extend_index_ttl(index, first_page, page_count)` (at most 25 pages per
   call) on the ones under the threshold. `index` is
   `{"Owner": "G…"}`, `{"Warehouse": "WH_…"}`, `{"Lender": "G…"}` or
   `{"LenderSales": "G…"}`.
5. Every call also refreshes the contract instance and `UnsettledSales`, so
   there is nothing separate to do for those.
6. **Nothing for the anchors or the claim bitmap.** Each warehouse's `Head`
   is rewritten by its daily anchor; `DayRoot` entries are temporary by
   design; `Claimed` chunks are left to archive and restored by the mint
   that reads them.

Why 25: it is the largest batch that stays well inside Testnet's
per-transaction limits even when every receipt in it carries several sale
records, and it was submitted at that size on Testnet (cost table below).
The binding limits are `tx_max_footprint_entries` = 400 (each receipt
needs 1 entry plus 1 per sale in its history) and
`tx_max_contract_events_size_bytes` = 16,384 (one `ttl_extended` event per
receipt).

## When an entry has been archived: the restore path

You will see one of these:

- a transaction simulation reports that the footprint includes archived
  entries,
- `stellar contract read --key-xdr …` returns nothing for a key the indexer
  knows exists,
- the keep-alive job's TTL read shows `live_until_ledger` in the past.

### Option A: let the next transaction restore it

Since protocol 23, `simulateTransaction` marks archived entries in the
footprint for automatic restoration, and the assembled transaction (JS SDK
`prepareTransaction` / `assembleTransaction`, or `stellar contract invoke`)
restores them as part of the call. The caller pays the restore: rent for
the minimum TTL, plus a write. For a 432-byte receipt that is about 0.07
XLM on mainnet today (120 days at 1,000/KB), or about 0.02 on Testnet
(7 days at 5,365/KB). Any business call works this way, including
`extend_ttl(receipt_id)`, which is also the cheapest because it restores
**and** extends in one transaction. The contract's own test host behaves the
same way (`ttl::an_archived_receipt_is_restored_intact_and_needs_extending_again`):
the receipt comes back byte-for-byte identical, with only the minimum TTL.

**So the normal restore is a single call:**

```bash
stellar contract invoke --id $CONTRACT_ID --source-account ops --network testnet \
  -- extend_ttl --receipt_id 42
```

### Option B: explicit restore, step by step

Use this when a call that touches the entry keeps failing, or to restore
something no call reaches cheaply, such as an index page.

1. **Build the storage key** as base64 XDR:

   ```bash
   # a receipt
   K=$(echo '{"vec":[{"symbol":"Receipt"},{"u64":"42"}]}' | stellar xdr encode --type ScVal)
   # the claim chunk holding deposit number 1,500 of a warehouse (1500 / 1024 = 1)
   K=$(echo '{"vec":[{"symbol":"Claimed"},{"symbol":"WH_KANO_01"},{"u64":"1"}]}' | stellar xdr encode --type ScVal)
   # a sale
   K=$(echo '{"vec":[{"symbol":"Sale"},{"u64":"7"}]}' | stellar xdr encode --type ScVal)
   # page 0 of a farmer's index (likewise WarehouseIndex/Symbol, LenderIndex, LenderSaleIndex)
   K=$(echo '{"vec":[{"symbol":"OwnerIndex"},{"address":"G…"},{"u32":0}]}' | stellar xdr encode --type ScVal)
   ```

2. **Restore it:**

   ```bash
   stellar contract restore --id $CONTRACT_ID --key-xdr $K --durability persistent \
     --source-account ops --network testnet
   ```

   It is idempotent: on a live entry it reports "No changes detected,
   transaction was a no-op."
3. **Extend it straight away.** A restored entry gets only the minimum TTL
   (7 days on Testnet, 120 on mainnet), so call `extend_ttl --receipt_id 42` for a receipt
   or sale, or `extend_index_ttl` for a page.
4. **Verify:** `stellar contract read --id $CONTRACT_ID --key-xdr $K` shows
   the value and its new `live_until_ledger`, and `get_receipt` returns the
   receipt.
5. **If the contract instance itself was archived** (every call fails), run
   `stellar contract restore --id $CONTRACT_ID` with no key, which restores
   the instance, and `stellar contract restore --wasm-hash <wasm_hash from
   deployments/testnet.json>` for the code, then make any state-changing
   call.

Steps 1, 2 and 4 were run against the current Testnet deployment (on live
entries) to confirm the key encodings and commands. Testnet's 7-day minimum
TTL and 175-day bumps mean nothing in this deployment can actually archive
for months, so the archived case was exercised in the contract's test host,
not on Testnet.

## Testnet is not mainnet: what transfers and what doesn't

Fetched with `stellar network settings` on 2026-10-03 from both networks
(Testnet via the SDF RPC, mainnet via `https://mainnet.sorobanrpc.com`).
Both run protocol 29.

**Identical on both:** every per-resource fee (instructions, disk reads,
writes, write bytes, events, transaction size, historical data), every
per-transaction and per-ledger limit (400 footprint entries, 16,384 event
bytes, 132,096 write bytes, 400M instructions), `max_entry_ttl` (3,110,400
ledgers = 180 days) and the persistent rent denominator (1,215). So
everything in the tables below except the rent column transfers as is.

**Different, and it matters:**

| setting | Testnet | mainnet | effect |
|---|---:|---:|---|
| rent per 1 KB (dynamic, see below) | **5,207** stroops | **1,000** stroops (the protocol floor) | mainnet rent is about **5.2× cheaper today** |
| `soroban_state_target_size_bytes` | 4.0 GB | 3.0 GB | input to the rent rate |
| live Soroban state (30-sample average) | 3.29 GB | 1.68 GB | input to the rent rate |
| `min_persistent_ttl` | 120,960 (7 days) | 2,073,600 (120 days) | a restored or newly created entry gets at least this much TTL. On mainnet a restore therefore also buys 120 days of rent. |
| `min_temporary_ttl` | 720 | 17,280 | not used here, since the contract has no temporary storage |

**The rent rate is not a constant.** Each network recomputes it from its
own live state size: below the target it rises linearly from −17,000 to
10,000 stroops per KB (with a floor of 1,000), and above the target it grows
5,000 times faster. Mainnet stays on the 1,000 floor until its state reaches
about 2.0 GB. The 30 samples fetched (about 2.7 hours of ledgers) grew from
1.667 to 1.693 GB. Don't read a trend into one window, but the floor could
end soon. At the 3 GB target the rate is 10,000 per KB, about 10× today and
about 2× Testnet. **Budget with the "mainnet at 10,000/KB" column as the
planning case, and the "mainnet today" column as the current spend.**

Rechecked on 2026-10-04: Testnet's rate had risen to 5,365, and mainnet,
still on the floor, held 1.78 GB of state, up 0.1 GB in a day.

Testnet numbers must never be quoted as mainnet cost. Today they overstate
rent about 5×, and they could understate it later.

**How the mainnet columns are derived.** Rent is
`entry bytes × rate × ledgers ÷ (1024 × 1215)` per entry, plus 2,542
stroops for each entry whose TTL is extended (the 2,500 write fee plus 48
bytes of TTL entry). The three native extends below checked that formula
against what Testnet actually charged, and it matched within 0.1% (68,825
predicted vs 68,864 charged, 333,800 vs 333,991, and 9,555,428 vs 9,560,863).
The mainnet columns take each measured Testnet fee and rescale only its rent
part by the ratio of the two rates. Everything else in the fee is the same
on both networks. The 1,000/KB mainnet rate was cross-checked by simulating an
extend on mainnet against a live entry. After removing the 15% safety margin
RPC adds to simulated rent, which showed up on both networks, it gives the
same rate. The settings were fetched with stellar-cli 27 against protocol 29
networks (the `data_version` 3 snapshots with 28.1.0, which warns the same
way). The CLI warns that it may miss newer settings, but the match against
charged fees shows nothing that affects rent was missed.

## Measured cost per operation, before and after

Two deployments, both measured with `scripts/measure_costs.py`, which
submits every transaction for real and reads back what the network charged:

- **Before:** `CBTFO2N4…KECW6KU` (now retired), `data_version` 1 (named-field storage,
  splits that retire the parent), measured 2026-10-03. Raw numbers in
  `deployments/testnet.v1.costs.json` and `testnet.v1.entries.json`.
- **After:** `CDLSGBNI…LZJW3` (now retired), `data_version` 2 (compact storage, splits in
  place, finished receipts skipped by the keep-alive), measured 2026-10-04.
  Raw numbers in `deployments/testnet.v2.costs.json` and
  `testnet.v2.entries.json`. The next deployment, `CB3PY2P5…NXKYFPEM` (also retired), was
  the same build plus a 64-byte cap on `deposit_id`, a single length check
  that does not change these costs measurably, so it was not re-measured.

Testnet's rent rate moved between the two runs, from 5,207 to 5,365 stroops
per KB. The Testnet column is what was actually charged. The mainnet columns
rescale each run's rent from the Testnet rate it was charged at, so they
compare like with like. All figures are XLM (1 XLM = 10,000,000 stroops), and
"change" is for mainnet today.

| operation | Testnet before → after | mainnet today before → after | mainnet at 10,000/KB before → after | change |
|---|---:|---:|---:|---:|
| mint — farmer's first receipt (creates owner index) | 1.7167 → 1.1445 | 0.3330 → 0.2166 | 3.2932 → 2.1297 | -35% |
| mint — farmer's later receipt | 1.3465 → 0.7624 | 0.2619 → 0.1454 | 2.5821 → 1.4176 | -44% |
| lock — full | 0.2253 → 0.1436 | 0.0452 → 0.0286 | 0.4305 → 0.2657 | -37% |
| lock — partial (splits) | 2.6185 → 0.7522 | 0.5070 → 0.1439 | 5.0241 → 1.3981 | -72% |
| exit — full | 0.0222 → 0.0228 | 0.0057 → 0.0056 | 0.0411 → 0.0411 | -1% |
| exit — partial (splits) | 2.4154 → 0.6314 | 0.4674 → 0.1208 | 4.6347 → 1.1735 | -74% |
| extend_due | 0.0010 → 0.0009 | 0.0010 → 0.0009 | 0.0010 → 0.0009 | -5% |
| unlock — partial (splits) | 2.6326 → 0.6258 | 0.5091 → 0.1198 | 5.0517 → 1.1631 | -76% |
| unlock — full | 0.0009 → 0.0009 | 0.0009 → 0.0009 | 0.0009 → 0.0009 | -5% |
| start_recovery | 0.4113 → 0.1992 | 0.0798 → 0.0379 | 0.7889 → 0.3705 | -53% |
| complete_recovery | 1.8642 → 1.5345 | 0.3614 → 0.2893 | 3.5764 → 2.8567 | -20% |
| record_settlement | 0.0277 → 0.0284 | 0.0070 → 0.0070 | 0.0512 → 0.0512 | -0% |
| extend_ttl — 1 receipt, TTL above threshold (no-op bump) | 0.0006 → 0.0006 | 0.0006 → 0.0006 | 0.0006 → 0.0006 | -0% |
| extend_ttl_batch — 25 receipts, TTL above threshold | 0.0066 → 0.0065 | 0.0066 → 0.0065 | 0.0066 → 0.0065 | -1% |
| extend_ttl_batch — 2 Exited receipts (skipped) | — → 0.0005 | — → 0.0005 | — → 0.0005 | new |
| extend_index_ttl — 2 pages | 0.0005 → 0.0005 | 0.0005 → 0.0005 | 0.0005 → 0.0005 | +0% |
| native extend — 1 receipt, +1 day (to 176 days) | 0.0071 → 0.0038 | 0.0016 → 0.0009 | 0.0135 → 0.0068 | -43% |
| native extend — 1 receipt, +5 days (to 180-day ceiling) | 0.0336 → 0.0168 | 0.0067 → 0.0033 | 0.0644 → 0.0310 | -50% |
| native extend — 25 receipts + deposit guards, to 180-day ceiling | 0.9583 → 0.5395 | 0.1859 → 0.1024 | 1.8384 → 1.0037 | -45% |

What changed and why:

- **Partial operations cost 72–76% less.** They now write one new receipt
  instead of two, and the receipt is half the size. A partial lock went from
  about 12× a full lock to 5×, and a partial exit from about 110× a full exit
  to 28×. It still creates a receipt, which a full exit does not.
- **A mint costs 44% less**, because the receipt it creates is 53% smaller.
  The deposit guard and index pages it also writes did not change.
- **`complete_recovery` saves only 20%.** Most of its cost is entries that
  did not shrink: the new `Sale` (smaller, but new), the lender-sale and
  buyer index pages, and `UnsettledSales`.
- Operations that rewrite entries in place (`exit` in full, `extend_due`,
  `unlock` in full, `record_settlement`) were already almost rent-free, and
  stay the same.
- **A keep-alive call naming finished receipts is a no-op.** The new
  `extend_ttl_batch — 2 Exited receipts (skipped)` row bumped nothing and
  emitted no events.

The after-run in raw stroops, with the resources each transaction declared:

| operation | fee (stroops) | of which rent | CPU instr. | write B | footprint RO / RW | events B | tx |
|---|---:|---:|---:|---:|---:|---:|---|
| mint — farmer's first receipt (creates owner index) | 11,444,574 | 11,404,321 | 2,369,820 | 1,836 | 5 / 8 | 584 | `030ef9239d30` |
| mint — farmer's later receipt | 7,624,329 | 7,583,957 | 2,436,305 | 1,920 | 5 / 8 | 584 | `8d5030da513d` |
| lock — full | 1,436,141 | 1,413,069 | 1,696,867 | 944 | 3 / 4 | 384 | `5982e7307534` |
| lock — partial (splits) | 7,521,509 | 7,476,376 | 2,692,155 | 2,756 | 2 / 10 | 888 | `352136e10b69` |
| exit — full | 228,310 | 211,688 | 1,473,120 | 524 | 4 / 2 | 364 | `38debeda987f` |
| exit — partial (splits) | 6,313,674 | 6,274,988 | 2,459,932 | 2,348 | 3 / 8 | 868 | `ce6942a21fcc` |
| extend_due | 9,279 | 0 | 886,367 | 536 | 2 / 1 | 424 | `4165a2f158a3` |
| unlock — partial (splits) | 6,257,777 | 6,219,546 | 2,151,680 | 2,748 | 1 / 9 | 984 | `944ccbcc3ee0` |
| unlock — full | 8,985 | 0 | 884,048 | 440 | 2 / 1 | 380 | `e443a7aed077` |
| start_recovery | 1,991,767 | 1,982,687 | 888,093 | 680 | 2 / 1 | 372 | `bc016e0ebffc` |
| complete_recovery | 15,344,999 | 15,304,455 | 2,342,249 | 2,128 | 3 / 9 | 556 | `722c758d37ac` |
| record_settlement | 284,132 | 263,557 | 1,570,592 | 596 | 4 / 3 | 568 | `e187806e16f3` |
| extend_ttl — 1 receipt, TTL above threshold (no-op bump) | 5,962 | 0 | 875,385 | 0 | 5 / 0 | 284 | `0c4174e6ec6a` |
| extend_ttl_batch — 25 receipts, TTL above threshold | 65,036 | 0 | 7,795,465 | 0 | 53 / 0 | 7,100 | `822598197e12` |
| extend_ttl_batch — 2 Exited receipts (skipped) | 4,640 | 0 | 866,294 | 0 | 5 / 0 | 0 | `3e5305f146a8` |
| extend_index_ttl — 2 pages | 5,393 | 0 | 819,473 | 0 | 5 / 0 | 0 | `6984641d6b34` |
| native extend — 1 receipt, +1 day (to 176 days) | 37,579 | 35,085 | 0 | 0 | 1 / 0 | 0 | `142e692c7816` |
| native extend — 1 receipt, +5 days (to 180-day ceiling) | 167,647 | 165,153 | 0 | 0 | 1 / 0 | 0 | `87fa07845235` |
| native extend — 25 receipts + deposit guards, to 180-day ceiling | 5,395,033 | 5,372,499 | 0 | 0 | 50 / 0 | 0 | `2bdfcb9a4d31` |

The rent formula still matches what was charged within 0.1%, using
Testnet's rate on the day (165,001 stroops predicted vs 165,153 charged for
the +5-day extend).

### Stored entry sizes

Size as rent is charged on it: the full `LedgerEntry` XDR, read back from
Testnet with `getLedgerEntries`.

| entry | before (B) | after (B) | change |
|---|---:|---:|---:|
| Receipt — Active, freshly minted | 912 | 432 | -53% |
| DepositIndex | 124 | 124 | +0% |
| Receipt — Locked | 1,076 | 528 | -51% |
| Receipt — Active, reduced in place by a split (before: split child) | 920 | 432 | -53% |
| OwnerIndex page 0 | 920 | 800 | -13% |
| Receipt — after a completed recovery | 1,312 | 652 | -50% |
| Sale — settled | 712 | 416 | -42% |
| Receipt — Exited, split off | — | 456 |  |

A fresh receipt's stored value went from 816 to 316 bytes; the remaining 116
bytes of each entry are the key and the ledger-entry envelope. The
`layout::a_receipt_is_stored_without_field_names_or_its_own_id` test fails
if a fresh receipt's value grows past 330 bytes. Index pages shrank only
because the farmer in this run had fewer receipts on page 0.

## Rent per bump length

Rent is linear in ledgers. So **the rent per receipt per month is the same
whatever the bump length**, and only the per-renewal overhead differs: 2,542
stroops per entry plus a share of one transaction (a native extend of 50
entries cost 22,534 stroops besides rent, about 900 per receipt). "Receipt +
guard" is the receipt plus its 124-byte `DepositIndex`, the two entries kept
alive per live receipt. The last column is the same figure before the
change.

| bump | network | rent per receipt + guard (556 B) | batch of 25 (one tx) | per receipt per month, all-in | before (per receipt per month) |
|---|---|---:|---:|---:|---:|
| 30 days | Testnet | 0.1248 | 3.1222 | 0.1249 | 0.2254 |
| 30 days | mainnet today | 0.0237 | 0.5941 | 0.0238 | 0.0438 |
| 30 days | mainnet at 10,000/KB | 0.2322 | 5.8066 | 0.2323 | 0.4323 |
| 90 days | Testnet | 0.3734 | 9.3367 | 0.1245 | 0.2250 |
| 90 days | mainnet today | 0.0700 | 1.7525 | 0.0234 | 0.0434 |
| 90 days | mainnet at 10,000/KB | 0.6955 | 17.3900 | 0.2319 | 0.4319 |
| 175 days (contract) | Testnet | 0.7255 | 18.1405 | 0.1244 | 0.2249 |
| 175 days (contract) | mainnet today | 0.1356 | 3.3934 | 0.0233 | 0.0433 |
| 175 days (contract) | mainnet at 10,000/KB | 1.3519 | 33.7997 | 0.2318 | 0.4318 |

**A 12-month bump is not possible on either network.** `max_entry_ttl` is
180 days, and the largest bump the network accepts is `max_entry_ttl − 1` =
3,110,399 ledgers. One more ledger is rejected as `Malformed`, and this was
hit on Testnet while measuring. A 6–12-month receipt needs at least one
renewal whatever we choose.

The contract's `extend_ttl` and `extend_ttl_batch` always bump to 175 days
(`ENTRY_TTL_EXTEND_TO`), and the deployment is immutable. A different bump
length is available without a new contract: the native
`ExtendFootprintTTL` operation, which is what `stellar contract extend
--key-xdr … --ledgers-to-extend N` sends, lets anyone extend any entry by any
length up to the ceiling. One transaction can carry up to 400 keys.

### What the bump length actually changes: leftover TTL

Since rent per month is fixed, the bump length decides one thing: how much
prepaid TTL is left when a receipt exits and stops being renewed. The mint
always prepays 175 days. Modelled per receipt (receipt + guard, keep-alive
cost only, mint excluded), with a daily job, at today's sizes and rates:

| strategy | receipt lives | renewals | TTL left over at exit | mainnet today | Testnet |
|---|---:|---:|---:|---:|---:|
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 180 d | 1 | 141 d | 0.1135 | 0.6056 |
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 270 d | 1 | 51 d | 0.1135 | 0.6056 |
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 365 d | 2 | 102 d | 0.2270 | 1.2113 |
| native, ~quarterly: under 14 days left, to 104 | 180 d | 1 | 86 d | 0.0709 | 0.3776 |
| native, ~quarterly: under 14 days left, to 104 | 270 d | 2 | 87 d | 0.1417 | 0.7552 |
| native, ~quarterly: under 14 days left, to 104 | 365 d | 3 | 83 d | 0.2126 | 1.1328 |
| native, ~monthly: under 14 days left, to 44 | 180 d | 1 | 26 d | 0.0245 | 0.1290 |
| native, ~monthly: under 14 days left, to 44 | 270 d | 4 | 29 d | 0.0981 | 0.5161 |
| native, ~monthly: under 14 days left, to 44 | 365 d | 7 | 27 d | 0.1718 | 0.9032 |

## TTL bump and renewal cadence (decided)

**The contract's 175-day bump (`extend_ttl_batch`), a daily job with the
30-day threshold described above, and an alert at 14 days left.** In practice
each live receipt is renewed once every 145 days.

Why not shorter native bumps, given that they leave less TTL over:

- **The saving is small.** On a 9-month receipt, monthly native bumps save
  about 0.015 XLM per receipt at today's mainnet rate, about 14% of
  keep-alive. At 2,000 deposits a month that is about 30 XLM a month (about
  300 at 10,000/KB).
- **The safety margin shrinks.** With 175-day bumps the job can be down for
  29 days before anything archives. With monthly bumps renewed at 14 days
  left, the margin is 13 days.
- **More moving parts.** A native extend touches only the keys it is given.
  The backend would have to list every `Sale`, `UnsettledSales`, the
  contract instance and the Wasm code itself, which `extend_ttl_batch` covers
  automatically. Native extends also emit no `ttl_extended` events to
  monitor.

Revisit if the mainnet rent rate moves well off the floor, or if the indexer
shows most receipts exiting in the first weeks after a renewal.

## Finished receipts are left to archive

`extend_ttl_batch` skips `Exited` and `Revoked` receipts, and the job never
sends them (see the job above). They archive about 175 days after their last
write. The indexer's event archive is their permanent record, and reading
one from the chain still works: the read restores it, at the reader's cost
(about 0.07 XLM on mainnet today for a 456-byte exited receipt).

**What this saves** depends on what the keep-alive would otherwise have
done. Measured against a job that renews every receipt of a deposit until
the deposit's last receipt exits, for a deposit drawn down in tranches (a
partial exit every two months, then a final exit), the receipts avoided are
the exited tranches. Each is a 456-byte receipt. The deposit guard is shared
with the live receipt and is kept alive either way.

| deposit life | partial exits (days) | renewals avoided | Testnet | mainnet today | mainnet at 10,000/KB |
|---|---|---:|---:|---:|---:|
| 270 days | 60, 120, 180, 240 | 2 | 0.9932 | 0.1860 | 1.8504 |
| 365 days | 60, 120, 180, 240, 300 | 4 | 1.9864 | 0.3719 | 3.7007 |

On a 9-month life the first two tranches would each have been renewed once.
Later tranches still have prepaid TTL when the deposit ends. Beyond the
deposit's life the saving is open-ended: a finished receipt kept alive
indefinitely costs, per year, 0.23 XLM on mainnet today, 2.31 XLM at
10,000/KB, and 1.24 XLM on Testnet. The
skip in the contract makes sure a careless job cannot run that up.

The financed and simple profiles below are not affected: their only exited
receipts finish at the end of the deposit's life.

## Monthly cost estimate

These figures assume every deposit mints a receipt that pays rent for 9–12
months (see the top of this document), under a storage strategy that is
still under review.

Each deposit profile's full lifetime cost: its transactions plus keep-alive
under the decided policy. Multiplied by the monthly intake, that is the
steady-state monthly spend once intake has been running for a full receipt
lifetime. Stated in XLM so it doesn't go stale with the XLM price. It does
move with the rent rate, so all three rates are shown.

| deposit profile | network | XLM per deposit, before → after | 200 deposits / month, before → after | 2,000 deposits / month, before → after |
|---|---|---:|---:|---:|
| Simple: mint, full exit, 9-month life | Testnet | 2.463 → **1.391** | 493 → **278** | 4,927 → **2,782** |
| Simple: mint, full exit, 9-month life | mainnet today | 0.478 → **0.265** | 96 → **53** | 957 → **529** |
| Simple: mint, full exit, 9-month life | mainnet at 10,000/KB | 4.725 → **2.587** | 945 → **517** | 9,450 → **5,174** |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life | Testnet | 10.782 → **4.592** | 2,156 → **918** | 21,564 → **9,185** |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life | mainnet today | 2.086 → **0.871** | 417 → **174** | 4,173 → **1,741** |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life | mainnet at 10,000/KB | 20.689 → **8.544** | 4,138 → **1,709** | 41,378 → **17,088** |

**Per deposit, after the change, to compare against fee income:**

| deposit profile | Testnet | mainnet today | mainnet at 10,000/KB |
|---|---:|---:|---:|
| Simple | 1.391 XLM | 0.265 XLM | 2.587 XLM |
| Financed | 4.592 XLM | 0.871 XLM | 8.544 XLM |

Plan with the 10,000/KB column; "mainnet today" is the current spend, at the
1,000/KB floor, which mainnet leaves at about 2.0 GB of state. On 2026-10-04
it held 1.78 GB, up from 1.68 GB the day before.

Not included: index-page keep-alive (`extend_index_ttl`, about 0.0005 XLM
per call), recoveries (about 1.76 XLM per default on Testnet and 0.33 at
today's mainnet rate, for `start_recovery` + `complete_recovery` +
`record_settlement`), and the classic transaction base fee (100 stroops per
transaction).

## Split in place (implemented)

A partial exit, lock or unlock used to retire the parent as `Status::Split`
and write two new children. Now the receipt keeps the remainder under its
own id. Its `weight_kg` and `bag_count` drop, and its status, lock and
recovery stay as they are. Only the portion acted on becomes a new receipt,
with `parent_id` naming the original. The partial-operation rows above show
the saving.

**The trade, accepted:**

- **A receipt's weight is no longer fixed for life.** Receipt #N's weight
  shrinks with each partial operation, so the chain holds only its current
  weight. Its history is `parent_id` plus the `split` events: each records
  the weight before, both weights after, and both ids (`remainder_id` is
  always the parent's own id). RPC keeps events for about 7 days, so **the
  indexer's own event archive is the permanent record**. Durable archiving,
  a one-day lag alert, gap checks after any outage and backups are a
  **correctness requirement**, spelled out in `docs/events.md` ("Indexer
  requirements").
- **The conservation invariant** is now "the receipts of a deposit always
  sum to its minted weight and bag count", and a receipt's weight never
  grows. `prop_split` and `prop_lifecycle` check both after every step, and
  that a sale is accounted for by the sold receipt plus whatever was split
  off it since. The 28 tests that asserted the old shape were rewritten.
- `Status::Split` no longer exists. Indexers that treated it as "superseded"
  need updating.
- It shipped as a new deployment with `DATA_VERSION` 2.

A partial exit still writes the exited portion as its own receipt. An exit
that writes no receipt at all, leaving the `exited` event as the only
record, would cost about the same as a full exit. That is a separate
decision and is not implemented.

## Smaller entries (implemented)

Receipts and sales are stored as positional records (`src/layout.rs`). The
public `Receipt` and `Sale` types, and everything the contract returns, are
unchanged. Field order and codes are the layout, so changing either means
bumping `DATA_VERSION`.

| change | done | what it cost us |
|---|---|---|
| Store positionally, without field names | yes | Raw storage is no longer self-describing: `stellar contract read` and explorers show unnamed fields, so tools decode through `data_version` and `layout.rs`. A small conversion cost on every read and write. |
| Unit enums (`status`, `weight_source`, recovery `resolution`) as `u32` codes; an absent lock or recovery as an empty `Vec` | yes | Readable variant names in raw storage. An unknown code aborts with `CorruptRecord` rather than being guessed. |
| Drop `receipt_id` (and a sale's `sale_id`) from the stored value | yes | A raw value no longer says which record it is; reads fill it in from the key. |
| Length caps on `deposit_id`, `loan_ref`, `sale_ref`, `settlement_ref` | yes, 64 bytes (`MAX_REF_LEN`), error `RefTooLong` | Longer refs must be shortened or hashed off-chain. The point is the bound: lenders are not registered, and each ref is copied into the lock, every recovery record, the sale and every receipt split off, so an unbounded ref let anyone inflate rent that the keep-alive job pays. |
| `commodity`, `grade` as `u32` codes | **no** | They are already symbols (`MAIZE_WHITE` is 20 bytes, `GRADE_1` 16), so codes would save about 20 bytes per receipt, about 0.0008 XLM per receipt-month on mainnet today. Codes need a registry that `mint` checks, which would make it reject unregistered commodities, a behaviour change. Not worth it. |
| `custodian`, `verifier` as registry indexes, or off-chain | **no** | Who attested the intake is the receipt's provenance. |
| Fixed-size byte arrays for refs | **no** | At the lengths in use they are larger than the strings (`BytesN<32>` is 40 bytes vs 16 for `"LOAN-M1"`). The cap gives the bound without making refs opaque. |

`deposit_id` is capped at the same 64 bytes. A registered custodian supplies
it, but in Phase 1 custodian keys are held by the backend, so registration
is not the safeguard it sounds like. It is stored in every receipt of the
deposit and is the deposit guard's key.
