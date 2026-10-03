# State archival and rent

Receipts live 6–12 months. Soroban storage is rented: every persistent entry
has a TTL, and when it runs out the entry is **archived**. Archived entries
are not deleted. Since protocol 23 they are restorable at any time. But until
someone pays to restore one, a transaction that touches it pays for the
restore, and an RPC read has to go through the restore path. This document
covers the TTL policy, the keep-alive job the backend must run, the restore
path, what each operation costs (measured on Testnet, converted to mainnet),
the recommended renewal strategy, and two proposals for cutting rent.

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
| `Receipt(id)` | every write to that receipt (lock, unlock, exit, split, extend_due, recovery), `extend_ttl`, `extend_ttl_batch` |
| `DepositIndex(deposit_id)`: the duplicate-mint guard | mint, `extend_ttl`, `extend_ttl_batch` (alongside its receipt) |
| `Sale(id)` | complete_recovery, record_settlement, and `extend_ttl(_batch)` on any receipt whose history names it, including every split descendant |
| `UnsettledSales` | complete_recovery, record_settlement, every `extend_ttl(_batch)` |
| owner / warehouse / lender / lender-sale **index pages** and their page counters | appending to the page while it is filling. **A full page (64 ids) is never written again.** It is kept alive only by `extend_index_ttl`. |
| registry entries (warehouse, custodian, verifier, operator) | being added, and every time they are checked (mint, exit, complete_recovery, record_settlement). An idle warehouse's registration can archive. That is harmless: the next mint restores it automatically. |
| contract instance + code | every state-changing call and every keep-alive call |

There is no temporary storage, so nothing in this contract can ever be lost
permanently through expiry.

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
   `Recovery`. Terminal receipts (`Exited`, `Revoked`, `Split`) are audit
   records. They may archive, and they restore on demand.
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

Why 25: it is the largest batch that stays well inside Testnet's
per-transaction limits even when every receipt in it carries several sale
records, and it was submitted at that size on Testnet (cost table below).
The binding limits are `tx_max_footprint_entries` = 400 (each receipt
needs 2 entries plus 1 per sale in its history) and
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
the minimum TTL, plus a write. For a 912-byte receipt that is about 0.15
XLM on mainnet today (120 days at 1,000/KB), or about 0.05 on Testnet
(7 days at 5,207/KB). Any business call works this way, including
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
   # its duplicate-mint guard
   K=$(echo '{"vec":[{"symbol":"DepositIndex"},{"string":"DEP-2026-0042"}]}' | stellar xdr encode --type ScVal)
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
networks. The CLI warns that it may miss newer settings, but the 0.1% match
against charged fees shows nothing that affects rent was missed.

## Measured cost per operation

Network: Testnet, contract `CBTFO2N4…KECW6KU`, all transactions from one run
of `scripts/measure_costs.py` on 2026-10-03, which submits every one of them
for real and reads back what the network charged. The raw numbers are in
`deployments/testnet.costs.json` and `deployments/testnet.entries.json`. 1 XLM
= 10,000,000 stroops.

| operation | Testnet fee (XLM) | of which rent | mainnet today (XLM) | mainnet at 10,000/KB | CPU instr. | write B | footprint RO / RW | events B | tx |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| mint — farmer's first receipt (creates owner index) | 1.7167 | 1.7127 | 0.3330 | 3.2932 | 2,374,182 | 1,992 | 5 / 8 | 584 | `ac2d02251b94` |
| mint — farmer's later receipt | 1.3465 | 1.3424 | 0.2619 | 2.5821 | 2,482,196 | 2,604 | 5 / 8 | 584 | `5642b72d89b9` |
| lock — full | 0.2253 | 0.2229 | 0.0452 | 0.4305 | 1,808,233 | 1,684 | 3 / 4 | 384 | `3b82d15bb0da` |
| lock — partial (splits) | 2.6185 | 2.6134 | 0.5070 | 5.0241 | 3,557,584 | 5,124 | 2 / 11 | 888 | `f835f5fcc67c` |
| exit — full | 0.0222 | 0.0205 | 0.0057 | 0.0411 | 1,566,873 | 1,012 | 4 / 2 | 364 | `386fb62d741f` |
| exit — partial (splits) | 2.4154 | 2.4110 | 0.4674 | 4.6347 | 3,293,943 | 4,480 | 3 / 9 | 868 | `edac10374fb8` |
| extend_due | 0.0010 | 0.0000 | 0.0010 | 0.0010 | 985,450 | 1,084 | 2 / 1 | 424 | `cef26191debf` |
| unlock — partial (splits) | 2.6326 | 2.6281 | 0.5091 | 5.0517 | 3,243,351 | 5,340 | 1 / 10 | 984 | `faa6e1ace3cc` |
| extend_ttl — 1 receipt, TTL above threshold (no-op bump) | 0.0006 | 0.0000 | 0.0006 | 0.0006 | 887,851 | 0 | 5 / 0 | 284 | `eee254bee101` |
| extend_ttl_batch — 25 receipts, TTL above threshold | 0.0066 | 0.0000 | 0.0066 | 0.0066 | 8,848,763 | 0 | 53 / 0 | 7,100 | `8cdc2d3470f1` |
| extend_index_ttl — 2 pages | 0.0005 | 0.0000 | 0.0005 | 0.0005 | 816,338 | 0 | 5 / 0 | 0 | `bf404ac9136f` |
| native extend — 1 receipt, +1 day (to 176 days) | 0.0071 | 0.0069 | 0.0016 | 0.0135 | 0 | 0 | 1 / 0 | 0 | `a8fd54fd90ec` |
| native extend — 1 receipt, +5 days (to 180-day ceiling) | 0.0336 | 0.0334 | 0.0067 | 0.0644 | 0 | 0 | 1 / 0 | 0 | `a7d66f30fceb` |
| native extend — 25 receipts + deposit guards, to 180-day ceiling | 0.9583 | 0.9561 | 0.1859 | 1.8384 | 0 | 0 | 50 / 0 | 0 | `f00ed6b9b64f` |
| start_recovery | 0.4113 | 0.4103 | 0.0798 | 0.7889 | 1,008,562 | 1,400 | 2 / 1 | 372 | `b4ce3d7391b3` |
| complete_recovery | 1.8642 | 1.8601 | 0.3614 | 3.5764 | 2,477,173 | 3,084 | 3 / 9 | 556 | `20caf89faae5` |
| record_settlement | 0.0277 | 0.0256 | 0.0070 | 0.0512 | 1,605,243 | 892 | 4 / 3 | 568 | `ea7f33b81f9c` |
| unlock — full | 0.0009 | 0.0000 | 0.0009 | 0.0009 | 978,145 | 920 | 2 / 1 | 380 | `d8dfbda0789c` |

What the table says:

- **Rent is almost the whole fee.** Every write that creates an entry pays
  for its first 175 days up front. A mint creates a receipt, a deposit guard,
  and sometimes index pages. Everything else (CPU, I/O, events) is
  thousandths of an XLM.
- **A partial operation costs about 12× its full counterpart for a lock and
  about 110× for an exit**, because it creates two new receipts and appends
  to four index pages. A full exit creates nothing. See the in-place split
  proposal below.
- **Keep-alive calls on healthy entries are nearly free.**
  `extend_ttl_batch` on 25 receipts whose TTL is above the threshold costs
  0.0066 XLM on either network. That is the daily job's cost on a quiet day.
  It used 53 of 400 footprint entries and 7,100 of 16,384 event bytes.
- `extend_due`, the full `unlock` and `start_recovery` mostly rewrite
  entries in place, so they pay little rent. `start_recovery` grows the
  receipt by the `Recovery` record, and the extra bytes pay rent for the
  receipt's remaining TTL.

### Stored entry sizes

Size as rent is charged on it: the full `LedgerEntry` XDR, read back from
Testnet with `getLedgerEntries`.

| entry | bytes |
|---|---:|
| Receipt — Active, freshly minted | 912 |
| DepositIndex | 124 |
| Receipt — Locked | 1,076 |
| Receipt — Active, split child | 920 |
| OwnerIndex page 0 | 920 |
| Receipt — after a completed recovery | 1,312 |
| Sale — settled | 712 |

A receipt starts at 912 bytes and grows as it picks up a `Lock` (+164), a
`Recovery` and a recovery-history record (1,312 after one completed
recovery). Each split child copies all of it, including the history.

## Rent per bump length

Rent is linear in ledgers. So **the rent per receipt per month is the same
whatever the bump length**, and only the per-renewal overhead differs: 2,542
stroops per entry plus a share of one transaction. Measured on Testnet, a
native extend of 50 entries cost 22,534 stroops besides rent, about 900
stroops per receipt. "Receipt + guard" is the 912-byte receipt plus its
124-byte `DepositIndex`, the two entries kept alive per receipt.

| bump | network | rent per receipt + guard | batch of 25 (one tx) | per receipt per month, all-in |
|---|---|---:|---:|---:|
| 30 days (~1 month) | Testnet | 0.2253 | 5.6342 | 0.2254 |
| 30 days (~1 month) | mainnet today | 0.0437 | 1.0941 | 0.0438 |
| 30 days (~1 month) | mainnet at 10,000/KB | 0.4322 | 10.8066 | 0.4323 |
| 90 days (~3 months) | Testnet | 0.6748 | 16.8726 | 0.2250 |
| 90 days (~3 months) | mainnet today | 0.1300 | 3.2525 | 0.0434 |
| 90 days (~3 months) | mainnet at 10,000/KB | 1.2955 | 32.3900 | 0.4319 |
| 175 days (contract's `extend_ttl`) | Testnet | 1.3117 | 32.7938 | 0.2249 |
| 175 days (contract's `extend_ttl`) | mainnet today | 0.2523 | 6.3101 | 0.0433 |
| 175 days (contract's `extend_ttl`) | mainnet at 10,000/KB | 2.5186 | 62.9664 | 0.4318 |

**A 12-month bump is not possible on either network.** `max_entry_ttl` is
180 days, and the largest bump the network accepts is `max_entry_ttl − 1` =
3,110,399 ledgers. One more ledger is rejected as `Malformed`, and this was
hit on Testnet while measuring. A 6–12-month receipt needs at least one
renewal whatever we choose.

The contract's `extend_ttl` and `extend_ttl_batch` always bump to 175 days
(`ENTRY_TTL_EXTEND_TO`), and the deployment is immutable. A different bump
length is still available without a new contract: the native
`ExtendFootprintTTL` operation, which is what `stellar contract extend
--key-xdr … --ledgers-to-extend N` sends, lets anyone extend any entry by any
length up to the ceiling. One transaction can carry up to 400 keys. The
measured native extend of 25 receipts and their guards is in the table
above.

### What the bump length actually changes: leftover TTL

Since rent per month is fixed, the bump length decides one thing: how much
prepaid TTL is left when a receipt exits and stops being renewed. The mint
always prepays 175 days. Modelled per receipt (receipt + guard, keep-alive
cost only, mint excluded), with a daily job:

| strategy | receipt lives | renewals | TTL left over at exit | mainnet today | Testnet |
|---|---:|---:|---:|---:|---:|
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 180 d | 1 | 141 d | 0.2108 | 1.0946 |
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 270 d | 1 | 51 d | 0.2108 | 1.0946 |
| contract `extend_ttl_batch`: renew under 30 days left, to 175 | 365 d | 2 | 102 d | 0.4217 | 2.1893 |
| native, ~quarterly: under 14 days left, to 104 | 180 d | 1 | 86 d | 0.1315 | 0.6824 |
| native, ~quarterly: under 14 days left, to 104 | 270 d | 2 | 87 d | 0.2631 | 1.3648 |
| native, ~quarterly: under 14 days left, to 104 | 365 d | 3 | 83 d | 0.3946 | 2.0472 |
| native, ~monthly: under 14 days left, to 44 | 180 d | 1 | 26 d | 0.0452 | 0.2329 |
| native, ~monthly: under 14 days left, to 44 | 270 d | 4 | 29 d | 0.1808 | 0.9314 |
| native, ~monthly: under 14 days left, to 44 | 365 d | 7 | 27 d | 0.3164 | 1.6300 |

## Recommendation: TTL bump and renewal cadence

**Keep the contract's 175-day bump (`extend_ttl_batch`), with the daily job
and 30-day threshold described above, and alert at 14 days left.** In
practice each live receipt is renewed once every 145 days.

Why not shorter native bumps, given that they leave less TTL over:

- **The saving is small.** On a 9-month receipt, monthly native bumps save
  about 0.03 XLM per receipt at today's mainnet rate, about 15% of
  keep-alive. At 2,000 deposits a month that is about 60 XLM a month (about
  600 at 10,000/KB). For receipts that live just past 175 days the saving is
  larger, 0.17 XLM each.
- **The safety margin shrinks.** With 175-day bumps the job can be down for
  29 days before anything archives. With monthly bumps renewed at 14 days
  left, the margin is 13 days.
- **More moving parts.** A native extend touches only the keys it is given.
  The backend would have to list every `Sale`, `UnsettledSales`, the
  contract instance and the Wasm code itself, which `extend_ttl_batch` covers
  automatically. Native extends also emit no `ttl_extended` events to
  monitor.

Revisit if the mainnet rent rate moves well off the floor, or if the indexer
shows most receipts exiting in the first weeks after a renewal. In either
case switch the job to native extends of about 90 days for receipts the
indexer expects to exit soon, and keep `extend_ttl_batch` for the rest.

## Monthly cost estimate

Each deposit profile's full lifetime cost: its transactions plus keep-alive
under the recommended policy. Multiplied by the monthly intake, that is the
steady-state monthly spend once intake has been running for a full receipt
lifetime. Stated in XLM so it doesn't go stale with the XLM price. It does
move with the rent rate, so all three rates are shown.

| deposit profile | network | XLM per deposit | 200 deposits / month | 2,000 deposits / month |
|---|---|---:|---:|---:|
| Simple: mint, full exit, 9-month life | mainnet today | 0.478 | 96 | 957 |
| Simple: mint, full exit, 9-month life | mainnet at 10,000/KB | 4.725 | 945 | 9,450 |
| Simple: mint, full exit, 9-month life | Testnet | 2.463 | 493 | 4,927 |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life (two receipts kept alive) | mainnet today | 2.086 | 417 | 4,173 |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life (two receipts kept alive) | mainnet at 10,000/KB | 20.689 | 4,138 | 41,378 |
| Financed: mint, partial lock, full unlock, partial exit, full exit, 12-month life (two receipts kept alive) | Testnet | 10.782 | 2,156 | 21,564 |

Not included: index-page keep-alive (`extend_index_ttl`, about 0.0005 XLM
per call), recoveries (about 2.3 XLM per default on Testnet and 0.45 at
today's mainnet rate, for `start_recovery` + `complete_recovery` +
`record_settlement`), and the classic transaction base fee (100 stroops per
transaction).

## Proposal: split in place instead of retiring the parent (not implemented)

**Today:** a partial exit, lock or unlock retires the parent as
`Status::Split` and writes two new children. Each child is a new 900–1,300
byte entry that prepays 175 days of rent, and each is appended to the owner
and warehouse indexes, and to the lender index when pledged.

**Proposal:** the parent keeps the remainder under its own id. Its
`weight_kg` and `bag_count` drop, and its status, lock and recovery stay as
they are. Only the portion being acted on becomes a new receipt, with
`parent_id` pointing at the parent. Lineage is preserved: every child still
names its parent, and the `Split` event already records each division
(parent weight before, both weights, both ids). The `remainder_id` in that
event becomes the parent's own id.

**Measured saving.** The change is about 10 lines in `split.rs`, because
callers only use `.taken` and the remainder's id. A copy of the crate with
the change was built outside the repo, and both versions ran the same
scenario in the test host. Rent ledger-bytes for each operation, applied to
the fees actually measured on Testnet:

| operation | today, Testnet | in place, Testnet | today, mainnet | in place, mainnet | saving |
|---|---:|---:|---:|---:|---:|
| lock — partial | 2.6185 | 1.4254 | 0.5070 | 0.2778 | 46% |
| exit — partial | 2.4154 | 1.2203 | 0.4674 | 0.2379 | 49% |
| unlock — partial | 2.6326 | 1.2126 | 0.5091 | 0.2364 | 54% |

Each partial operation also writes one fewer entry and about 920–1,150 fewer
bytes. It also stops the tree from growing: today every partial operation
leaves a retired parent that still takes up index slots and, while it
lives, rent.

**What we would lose or have to change:**

- **Receipt immutability.** Today a receipt's weight never changes, and a
  holder of receipt #N knows exactly what it represents. In place, the
  parent's weight shrinks over time, so the history is reconstructed from
  `Split` events rather than read from retired records. RPC keeps events for
  about 7 days, so this relies on the indexer.
- **The conservation invariant** becomes "the live receipts of a deposit sum
  to its minted weight" instead of "the leaves of the split tree sum to the
  root". The property tests carry over, but 28 existing tests assert the old
  shape: the parent becomes `Split`, two new ids are created, and
  `leaves()` excludes the parent. They would be rewritten.
- `Status::Split` would no longer be produced. Indexers that treat it as
  "superseded" need updating.
- Because the contract is immutable (`docs/upgrade.md`), this ships as a new
  deployment with a `DATA_VERSION` bump.

Even in place, a partial exit still creates one receipt, so it would cost
about 55× a full exit instead of about 110×. Going further, a partial exit
could create no receipt at all: the parent shrinks and the `Exited` event is
the record. That would cost about the same as a full exit. The cost is that
the exited portion has no on-chain receipt to query, only an event, which
again relies on the indexer. That is a product decision, not a technical one.

## Proposal: smaller entries (not implemented)

Breakdown of a freshly minted receipt: 912 bytes, of which the value is 816.
**404 of those 816 bytes are field names.** `#[contracttype]` structs are
stored as a map keyed by field-name symbols. Savings below are per receipt,
using today's mainnet rate, with Testnet in brackets. "At mint" is the rent
prepaid for the first 175 days.

| change | bytes saved | % of entry | per receipt-month | at mint | what we lose |
|---|---:|---:|---:|---:|---|
| **Store positionally** (a private tuple-struct or `Vec` layout for storage, and keep the public `Receipt` type for reads and the ABI) | 404 | 44% | 0.0168 (0.0877) | 0.0982 (0.5113) | Raw storage is no longer self-describing: `stellar contract read` and explorers show unnamed fields, so tools must decode through `DATA_VERSION`. Small conversion cost on every read and write. |
| Shorter field names (1 character) instead of positional | 152 | 17% | 0.0063 (0.0330) | 0.0369 (0.1924) | Unreadable field names, both in storage and, if the public type is not kept separate, in the ABI. Strictly worse than positional. |
| Unit enums (`status`, `weight_source`) as `u32`, and the empty `lock` and `recovery` as void | 72 | 8% | 0.0030 (0.0156) | 0.0175 (0.0911) | Readable variant names in raw storage. Needs explicit conversion code and a check that rejects unknown codes. |
| Drop `receipt_id` from the value (it is already in the key) | 32 | 4% | 0.0013 (0.0069) | 0.0078 (0.0405) | A raw value no longer says which receipt it is. Reads fill it in from the key. |
| `commodity`, `grade` as `u32` codes instead of `Symbol` | 20 | 2% | 0.0008 (0.0043) | 0.0049 (0.0253) | They are already symbols (`MAIZE_WHITE` = 20 B, `GRADE_1` = 16 B), so there is little left to gain. Codes would need an on-chain or published registry, and lose self-description. |
| `custodian`, `verifier` as a `u32` registry index instead of an address | 72 | 8% | 0.0030 (0.0156) | 0.0175 (0.0911) | A new registry of index↔address. Deregistering an attester must never reuse an index, or old receipts would point at the wrong party. |
| Move `custodian` and `verifier` off-chain entirely (event only) | 124 | 14% | 0.0052 (0.0269) | 0.0301 (0.1569) | **Not recommended.** Who attested the intake is the receipt's provenance, and it would then rest on the indexer. |
| **Combined:** positional + enums + `receipt_id` + commodity/grade codes | 508 | 56% | 0.0212 (0.1102) | 0.1235 (0.6429) | All of the above except the attester change. |

**Fixed-size byte arrays for `loan_ref`, `sale_ref`, `settlement_ref`** (and
`deposit_id`) do not save space at the lengths we use today. A `String`
costs 8 bytes plus the text rounded up to 4. `BytesN<16>` costs 24 and
`BytesN<32>` costs 40. Measured: `"LOAN-M1"` = 16 B, `"LN-2026-000123"` =
24 B, and a 36-character UUID string = 44 B. So `BytesN<16>` (a binary UUID)
saves 20 bytes per ref only if the refs really are UUID strings, while
`BytesN<32>` (a hash of an off-chain document) saves nothing and costs more
for short refs. The ref appears in the `Lock`, every `Recovery`, and the
`Sale`, and every split child copies it.

The real argument for fixed-size refs is **a bound, not a saving.** None of
these strings has a length cap today, so a caller can make every entry that
stores one, and every split child that copies it, arbitrarily large, up to
the 64 KB entry limit. Whoever runs the keep-alive pays that rent. Whatever
type is chosen, a length cap (for example 64 bytes) should be added. The
cost is that refs become opaque bytes on-chain (a hash, or a binary UUID) and
must be resolved off-chain. A hash has the side benefit of not exposing loan
or bank identifiers publicly.

**What should stay on-chain:** weight, bags, moisture, warehouse, owner,
status, lock and recovery state, and the attesters. Those are what the
contract enforces or what a third party needs to trust a receipt. Interest
rates and loan terms already live off-chain.
