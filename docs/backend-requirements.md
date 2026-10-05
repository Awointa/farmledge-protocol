# Backend requirements

What the protocol needs from the Farmledge backend to be correct. These are
**correctness requirements**: missing one loses data or verifiability, and
is treated as a bug with the same priority as a contract bug.

**Conformance.** The nine invariants these requirements add up to (README,
"Contract frozen") are specified one by one, with an executable check for
each, in [`conformance/`](../conformance/README.md). A backend conforms
when `conformance/farmledge_conformance.py` passes all nine on its export,
offline and with `--rpc`.

| | applies | detail |
|---|---|---|
| 1. Indexer: archive every event durably, alert when a day behind, check for gaps after outages, back up | now | `docs/events.md` ("Indexer requirements") |
| 2. Keep-alive job: daily, 175-day bump, alert at 14 days left, never send `Exited` or `Revoked` ids | now | `docs/archival.md` ("The keep-alive job") |
| 3. Anchoring job | `data_version` 3 | below |
| 4. Farmer proof delivery | `data_version` 3 | below |
| 5. Minted lots: the freeze, and governed on-chain after | `data_version` 3 | below |
| 6. Cutover | **not to be built** (reference only) | below |

Items 3–6 belong to the A+B design in `docs/storage-strategy.md`. **The
contract side is built** (`data_version` 3, deployed to Testnet); **the
backend side is not built yet.** Until it is, nothing may mint on v3 outside
tests.

**v3 is the only contract the backend talks to.** **No dual-contract support is to be built — Testnet abandoned v2 and mainnet launches on v3.** The v2 Testnet
deployment held only test data and is retired; nothing reads, indexes or
keeps it alive. Item 6 is kept only as a reference.

## 3. Anchoring job

- **Once a day per warehouse,** at a fixed UTC cutoff, build that day's tree
  from Postgres: one deposit leaf per intake, one dispatch leaf per
  dispatch, one balance leaf per active lot, in the leaf format of
  `docs/storage-strategy.md`. That includes lots governed by an on-chain
  receipt, with `receipt_id` set.
- **Every leaf gets a fresh random 32-byte salt,** stored with the leaf and
  never reused.
- **Submit the anchor** (`anchor(custodian, warehouse_id, day, root,
  leaves)`) signed by a custodian registered for the warehouse, and confirm
  it on-chain: `get_anchor` returns the head computed locally,
  `sha256(prev_head ‖ day ‖ leaves ‖ root)`. Only then is the day anchored.
- **Persist every leaf, salt, proof and root** before delivering anything.
  They fall under the indexer's archive and backup rules: the leaf data is
  stored nowhere else.
- **A missed anchor is an urgent alert, not a routine one.** Nothing in a
  warehouse can mint until its latest day is anchored, and once its last
  root expires (30 days without an anchor) nothing can mint at all. During a
  single-warehouse trial a quietly failing job blocks every pledge, with no
  obvious cause. So:
  - **page** the on-call engineer if any warehouse's day is not anchored
    within **2 hours** of the cutoff, and **escalate** at **24 hours**;
  - raise a **separate urgent alert** for any warehouse with **no anchor in
    7 days**, well before the 30-day limit;
  - **in the pledge flow,** a mint refused with `StaleLeaf` or `NoAnchor`
    is reported as "the warehouse's books are not anchored for today",
    naming the warehouse and its last anchored day, never as a generic
    failure.
- **Anchor days strictly in order.** The contract accepts a day only if it
  is after the last anchored one and not after the ledger's current UTC
  day. A missed day can still be anchored late, but only until a later day
  is: never skip one to catch up.
- **Deposit numbers** are a per-warehouse sequence. They are assigned once,
  at intake, and are never reused or reassigned. One physical lot has
  exactly one number. Each day's tree must hold one deposit leaf per new
  number, with no gaps. A reconciliation check enforces both.

## 4. Farmer proof delivery

Automatic, never on request. For every deposit and every dispatch:

1. **At the counter,** the printed slip carries a QR code with the leaf's
   data and salt (the proof cannot exist yet, because the day's tree is
   built at the cutoff).
2. **Within 24 hours of the cutoff,** once the anchor is confirmed on-chain,
   the farmer receives the full proof bundle (format in `docs/events.md`,
   "Farmer proofs"):
   - by SMS, as a link to a page that shows the bundle as a QR code and a
     downloadable file, needs no login, and checks the proof **in the
     browser against a public Stellar RPC**, never against Farmledge's API.
     The page is a convenience: the bundle file is the proof, and anyone
     can check it without Farmledge using the standalone script in
     `docs/events.md`;
   - printed as a QR code at the warehouse, on request, on the next visit
     and on every dispatch slip.
3. **Every month,** a statement with a balance proof for each active lot, so
   the farmer always holds a proof less than 30 days old that can be checked
   against the chain with one call.

Delivery is part of the anchoring job's definition of done:

- **Alert** if any leaf's bundle has not been delivered within 24 hours of
  its anchor.
- **Retry** failed SMS, and record a delivery receipt for each bundle.
- **Publish** the standalone verifier (`scripts/verify_proof.py`, also
  reproduced in `docs/events.md`) and the bundle format, so lenders and
  farmers' representatives can check proofs without Farmledge. **No
  verification path may depend on Farmledge's API, database or website.**
- A bundle older than 30 days needs `later_days` (the `anchored` events
  after its day) to be checked: serve them publicly from the event archive,
  and include them in every monthly statement. They need not be trusted;
  the chain checks them.

## 5. Minted lots: the freeze, and governed on-chain after

The contract accepts a balance leaf only from the warehouse's latest
anchored day, but **it cannot see a dispatch recorded after that day's
cutoff.** Freshness in that window is this backend's invariant:

> No dispatch of a lot may be recorded between the cutoff of the anchored
> day its mint uses and the moment that mint is confirmed or definitively
> abandoned.

- **Freeze first.** In one database transaction: lock the lot's row; refuse
  if any dispatch of it is recorded after the latest anchored day's cutoff
  (mint after the next anchor instead); set `frozen_for_mint`; write the
  mint request as `pending`. Build the mint transaction only after this
  commits. **If the freeze cannot be written, nothing is submitted.**
- **Dispatch checks the freeze** in the same transaction that records the
  dispatch, and refuses while it is set. If it cannot read the freeze state
  it refuses: fail closed.
- **Store the signed transaction and its hash before submitting,** with a
  5-minute `max_time`.
- **Confirmed:** record the `receipt_id`; from then on every dispatch of
  the lot goes through the contract's `exit`, never off-chain, and its
  balance leaves carry the receipt id. The freeze is never lifted.
- **Unfreeze only on a definite failure:** `FAILED`, never submitted, or
  `max_time` passed and RPC confirms the transaction is absent.
- **After a crash,** resolve every `pending` request before dispatch
  reopens for its lot: no stored hash, unfreeze; a stored hash, look it up
  with `getTransaction` and complete or unfreeze as above. Never unfreeze
  while the transaction could still land.
- **Reconcile, after every anchor and for every `minted` event,** and page
  on any hit (details in `docs/storage-strategy.md`, "The freshness
  invariant"):
  - no `anchored` event before the mint's ledger has a day later than
    `minted.anchor_day`;
  - no dispatch of the lot is recorded, in the database or as a dispatch
    leaf in later anchored books, between `anchor_day`'s cutoff and
    `minted_at`;
  - the lot's first balance leaf after the mint carries the receipt id, and
    its `kg` equals the receipt's weight less any on-chain exits.
- **Alert** whenever an anchored balance for a minted lot differs from its
  receipt's on-chain weight. That means grain moved outside the contract.

## 6. Cutover (reference only: not to be built)

**No dual-contract support is to be built — Testnet abandoned v2 and mainnet launches on v3.** Nothing in this section is to be implemented. It is kept for a future
deployment that ever has to start on an older contract holding real lots:
then, and only then, the following applies.

Once per switch of contract; the procedure, failure modes and the mainnet
differences are in `docs/storage-strategy.md` ("Pre-claiming at cutover"
and "Migration path"). The backend's part:

- **Stop minting on the old contract on-chain** by removing its verifiers,
  and confirm before building the pre-claim list.
- **Build the list from the chain:** every `Active`, `Locked` or `Recovery`
  receipt of the old contract, mapped to its lot's `deposit_no`. Every live
  receipt maps to exactly one number.
- **Diff** the list against the old contract's live receipts and against
  the lots in v3's first books with `receipt_version` set. Any difference
  stops the cutover.
- **Publish, review, submit** (`preclaim`, 25 a call), then **verify**
  with `is_claimed` and the `preclaimed` events.
- **Gate:** no v3 mint for a warehouse until its pre-claim is verified.
- **For the whole overlap:** index and keep alive both contracts, and
  reconcile every v3 `minted` event against the old contract's live
  receipts daily.
