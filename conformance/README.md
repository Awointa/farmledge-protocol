# Backend conformance kit

The receipt contract (`data_version` 3) is frozen, and nine of the rules it
relies on are ones it cannot check. The backend is the only thing standing
behind them. This kit states each one precisely and checks any backend
against it: offline, from one JSON export of the backend's records, and
against the chain, through any public Stellar RPC.

A backend conforms when `farmledge_conformance.py export.json --rpc <url>`
prints `PASS` for all nine. Run it in the backend's CI on a fixture
export, and against its real export daily.

| file | |
|---|---|
| `farmledge_conformance.py` | the checks, I1–I9. Python 3.8+, standard library only; hashing and XDR from `scripts/verify_proof.py` |
| `example.py`, `example.export.json` | a synthetic export that conforms, and the broken variants the self-tests use, one per missing enforcement |
| `testnet_example.py`, `testnet.export.json` | a minimal conforming backend played against the deployed contract on Testnet, and its export |
| `test_conformance.py` | the self-tests |

```bash
python3 conformance/farmledge_conformance.py export.json                       # offline
python3 conformance/farmledge_conformance.py export.json --rpc https://soroban-testnet.stellar.org
python3 -m unittest conformance/test_conformance.py                            # self-tests, offline
FARMLEDGE_TESTNET=1 python3 -m unittest conformance/test_conformance.py        # and against Testnet
python3 conformance/testnet_example.py                                         # refresh testnet.export.json
```

Times are Unix seconds. A **day** is a UTC day number (seconds ÷ 86,400),
and its **cutoff** is the end of it, `(day + 1) × 86,400`, when its books
close.

## The nine invariants

Each invariant names the place in `docs/backend-requirements.md` that
specifies the enforcement, what the kit checks, and the tests that pin the
check. **Every check is shown to bite**: each invariant's test builds the
export a backend lacking that one enforcement would produce, and asserts
exactly which invariants fail. Disabling any check makes its tests fail.

### I1. Freshness: freeze a lot before minting it

**Rule.** No dispatch of a lot may be recorded between the cutoff of the
anchored day its mint uses and the moment the mint is confirmed or
definitely abandoned. The freeze is written before the mint is submitted,
and a request is never left pending past its `max_time`.
**Why.** The contract accepts a balance leaf only from the latest anchored
day, but cannot see a dispatch after that day's cutoff
(`docs/storage-strategy.md`, "The freshness invariant").
**Enforced in:** backend §5.
**Check.** For every mint request: `frozen_at ≤ submitted_at`; no
off-chain dispatch of its lot recorded from `frozen_at` (and, for a mint
that landed, from its anchor's cutoff) to `resolved_at`; nothing pending
an hour past `max_time`.
**Pinned by:** `test_i1_without_the_freeze_a_stale_mint_is_caught`. A
backend without the freeze is caught twice: by I1, and by I6 when the
anchored balance stops matching the receipt.

### I2. Pre-claim gate: no mint before the pre-claim list is complete

**Rule.** No mint in a warehouse is submitted before that warehouse's
pre-claim is recorded as verified, **even when there is nothing to
claim** (the gate is live code, dormant on Testnet). When a deployment
starts beside an older contract holding live lots, every one of them is
pre-claimed on-chain and none is minted here.
**Why.** A lot left unclaimed can be minted twice, on two contracts
(audit finding A19).
**Enforced in:** backend §6.
**Check.** Every mint request's `submitted_at` ≥ its warehouse's
`preclaim.verified_at`. With no older contract, no `preclaimed` event in
the backend's warehouses. With one, a `preclaimed` event for each live lot,
the bit set on-chain (`--rpc`), and no mint of a live lot.
**Pinned by:** `test_i2_a_mint_before_the_preclaim_gate_is_caught`,
`test_i2_a_live_lot_on_the_older_contract_left_unclaimed_is_caught`,
`test_i2_a_preclaimed_lot_minted_anyway_is_caught`,
`test_a_conforming_backend_beside_an_older_contract_passes`; on-chain,
`test_i2_a_preclaim_not_on_chain_is_caught_only_on_chain`.

### I3. One lot, one deposit number, forever

**Rule.** Per warehouse, deposit numbers are assigned once at intake,
never reused or reassigned, and run 1, 2, 3… without a gap. Each has
exactly one deposit leaf, on its intake day.
**Why.** The contract's duplicate guard is keyed on `(warehouse_id,
deposit_no)`; two numbers for one lot would let it mint twice (A2).
**Enforced in:** backend §3.
**Check.** The numbers are exactly `1..N`; one deposit leaf each, on the
intake day; no deposit leaf for an unknown number.
**Pinned by:** `test_i3_a_gap_in_deposit_numbers_is_caught`.

### I4. Complete, honest daily books, each leaf with a fresh salt

**Rule.** Each day's tree holds one deposit leaf per intake, one dispatch
leaf per dispatch, and one balance leaf per lot with grain left at the
cutoff, each matching its lot, ordered by (kind, deposit_no, at), with a
fresh random 32-byte salt. Grade and commodity are valid Soroban `Symbol`s.
**Why.** The anchors commit to whatever the books say; only complete
books make the commitment mean anything, and an unsalted or reused salt
lets a leaf hash be guessed.
**Enforced in:** backend §3.
**Check.** Each exported day's leaves equal the books derived from the
lots and dispatches; fields match the lot; day and index match the leaf's
place; salts are 32 bytes and never repeat; the leaves hash to the
`anchored` root and count, and (`--rpc`) to the root stored on-chain.
**Pinned by:** `test_i4_incomplete_books_are_caught`,
`test_i4_a_reused_salt_is_caught`; on-chain,
`test_i4_i5_i7_consistently_forged_books_are_caught_only_on_chain`.

### I5. Every warehouse anchored daily, in order, never skipping, never missing

**Rule.** Every day of every warehouse is anchored, in order; a missed
anchor is an urgent page at 2 hours.
**Why.** Nothing in a warehouse can mint until its latest day is anchored,
and a skipped day can never be anchored afterwards.
**Enforced in:** backend §3.
**Check.** Anchored days run without a gap from the first to the latest;
the last day whose cutoff passed more than 2 hours before `as_of` is
anchored; every `prev_head` links to the previous head, every head
recomputes, and (`--rpc`) the result is the head on-chain.
**Pinned by:** `test_i5_a_skipped_day_is_caught`,
`test_i5_a_missed_anchor_is_caught`; on-chain,
`test_i4_i5_i7_consistently_forged_books_are_caught_only_on_chain`.

### I6. A minted lot leaves only through the contract's `exit`

**Rule.** Once minted, every dispatch of the lot is an on-chain `exit`;
its balance leaves name the receipt; and its anchored balance always
equals what its receipts hold.
**Why.** Off-chain dispatch of pledged grain is invisible to the lender.
**Enforced in:** backend §5.
**Check.** No off-chain dispatch after the mint; every exit has its
`exited` event; every later balance leaf carries the receipt id and
`receipt_version` 3, and its weight equals the live weight of the receipt
and everything split off it, from the event archive.
**Pinned by:** `test_i6_a_minted_lot_dispatched_off_chain_is_caught`.

### I7. Every farmer's proofs delivered, and verifiable without Farmledge

**Rule.** Every leaf's bundle is delivered within 24 hours of its anchor,
carrying exactly that leaf, and it verifies against the chain with the
standalone verifier.
**Why.** An unpledged lot exists only in the books and the farmer's
proofs.
**Enforced in:** backend §4.
**Check.** A delivery per leaf, within 24 hours; the bundle's leaf is the
leaf; its proof folds to the anchored root; (`--rpc`) `verify_proof.py`
accepts it against the chain.
**Pinned by:** `test_i7_an_undelivered_proof_is_caught`,
`test_i7_a_late_proof_is_caught`; on-chain,
`test_i4_i5_i7_consistently_forged_books_are_caught_only_on_chain`.

### I8. Every event archived before the cursor moves

**Rule.** The archive covers every ledger from the deployment's
`start_ledger` to the cursor, holds each event once, and reconciles.
**Why.** `split` and `anchored` events are the only lasting record of
earlier weights and past roots.
**Enforced in:** `docs/events.md`, "Indexer requirements".
**Check.** The indexed ledger ranges have no gap; no event twice; every
receipt's weight equals its minted or split-off weight less what was split
from it; (`--rpc`) every event RPC still holds in the range is archived.
**Pinned by:** `test_i8_an_indexer_gap_is_caught`; on-chain,
`test_i8_an_event_missing_from_the_archive_is_caught_only_on_chain`.

### I9. Minted receipts kept alive, finished ones never sent

**Rule.** The keep-alive job never names an `Exited` or `Revoked`
receipt, never more than 25 per call, and every live receipt keeps more
than 14 days of TTL.
**Enforced in:** `docs/archival.md`, "The keep-alive job".
**Check.** No submission names a receipt finished before it; at most 25
ids; (`--rpc`) every `Active`, `Locked` or `Recovery` receipt has at least
14 days of TTL left.
**Pinned by:** `test_i9_a_keepalive_naming_an_exited_receipt_is_caught`.

## The export

One JSON object. Every field is required unless marked optional.

| field | |
|---|---|
| `format` | `1` |
| `contract_id`, `network` | the deployment checked |
| `as_of` | when the export was taken |
| `warehouses` | `[{"warehouse_id"}]`: the warehouses this backend operates |
| `lots` | `[{"warehouse_id", "deposit_no", "deposit_id", "owner", "commodity", "grade", "weight_source", "bags", "moisture_bps", "custodian", "verifier", "kg", "intake_at"}]` |
| `dispatches` | `[{"id", "warehouse_id", "deposit_no", "kg", "recorded_at", "via": "off_chain" \| "exit", "receipt_id"` (for an exit)`}]` |
| `days` | `[{"warehouse_id", "day", "leaves": [leaf, …]}]`, leaves in tree order, in the leaf format of `docs/events.md` |
| `mint_requests` | `[{"id", "warehouse_id", "deposit_no", "anchor_day", "frozen_at", "submitted_at"` (optional, null if never sent)`, "max_time", "resolved_at"` (null while pending)`, "outcome": "minted" \| "failed" \| "abandoned" \| "pending", "receipt_id"}]` |
| `preclaim` | `{"old_contract": null \| {"contract_id", "live_lots": [{"warehouse_id", "deposit_no", "receipt_id"}]}, "verified_at": {warehouse_id: time}}` |
| `deliveries` | `[{"warehouse_id", "day", "index", "channel", "delivered_at", "bundle"}]`, the bundle as in `docs/events.md` |
| `keepalive_submissions` | `[{"submitted_at", "receipt_ids": [...]}]` |
| `receipts` | the backend's projection: `[{"receipt_id", "status", "weight_kg", "deposit_no", "warehouse_id"}]` |
| `indexer` | `{"start_ledger", "cursor", "ledger_ranges": [[from, to], …], "events": [{"id"` (the RPC event id)`, "ledger", "tx", "at", "type", "fields"}]}`: every event of the contract, its topics and data decoded by field name as in `docs/events.md` |

`farmledge_conformance.archive_event` turns a `getEvents` record into an
archive record, so an indexer can use the same decoding.

## Verified

- **Offline:** the conforming example passes; each of the 14 broken
  variants fails exactly the invariants its test names; disabling any
  check makes its tests fail.
- **On Testnet,** 2026-10-05, against `CBW6TA5U…KIT4UAIN`:
  `testnet_example.py` registered a warehouse, anchored two days, froze and
  minted a lot from the latest one, exited 500 kg on-chain, ran a
  keep-alive, archived all 258 events of the contract from RPC, and
  the export passed all nine checks with `--rpc`. Three tampered copies
  of it, each still passing offline, were each caught only by the chain.
