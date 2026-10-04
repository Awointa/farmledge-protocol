# Storage strategy: what to keep on-chain

**Status: design evaluation. Nothing here is implemented.** The contract and
the current deployment (`CB3PY2P5…NXKYFPEM`) are unchanged.

Rent is the cost that matters, and the current design pays it on every
deposit: each one mints a receipt and a deposit guard that live for 9–12
months. This document compares that with four alternatives: what each
stores, what it costs (measured on Testnet, converted to mainnet), and,
just as important, what a party who does not trust Farmledge can still
verify.

## The short version

| design | per deposit, mainnet today | per deposit, at 10,000/KB | what a lender loses compared with today |
|---|---:|---:|---|
| Current | 0.273 XLM | 2.667 XLM | nothing |
| A: mint on demand | 0.072 | 0.691 | the on-chain record of every unpledged deposit |
| A+B: + daily anchoring | 0.075 | 0.719 | less than A: unpledged deposits are committed to daily, and provable with a Merkle proof |
| A+B+C: + aggregated lots | 0.064 | 0.611 | as A+B, plus per-lot lineage and concurrency headroom |
| D: minimal chain | 0.034 | 0.319 | the receipt itself: on-chain enforcement ends at the pledge |

**Recommendation: A+B**, with mint-on-demand that must prove the deposit is
in an anchored day. It removes 73% of the cost while keeping full on-chain
enforcement for every pledged lot, which is where third-party trust is
needed. See [Recommendation](#recommendation) and
[Migration path](#migration-path).

## How this was measured

- **Current design:** the `data_version` 2 measurements in
  `docs/archival.md` (Testnet, `CDLSGBNI…`, the same code as today's
  deployment apart from the `deposit_id` cap).
- **A, B, C, D:** a **throwaway prototype contract**, deployed to Testnet
  only to measure them: `CBSD3ZLE6Y6IKNKYGHG663DWZ6ZESST4V3QHICVTKRWHUCB2ZKRSB6S5`.
  Its on-chain metadata says "THROWAWAY PROTOTYPE … Not a Farmledge
  deployment. Do not use." It is not in `deployments/` and nothing should
  call it. It is the current contract plus one module implementing A's
  mint, B's anchoring, C's aggregated lots and D's pledge-from-anchor, each
  exercised in the test host first (including rejecting a forged leaf and a
  double pledge) and then submitted for real. Fees are what Testnet charged
  on 2026-10-04.
- **Rates:** Testnet 5,365 and mainnet 1,000 stroops per KB on 2026-10-04,
  and 10,000/KB as the planning case (see `docs/archival.md` for why the
  rate moves). Each measured fee's rent part is rescaled to each rate. The
  rent formula has matched charged fees within 0.1%.
- **One computed figure:** B's day root is measured as a persistent entry,
  but the recommended design keeps it in *temporary* storage for 30 days.
  Temporary rent is half persistent rent by the network's own denominators,
  so that cost is computed rather than charged.

**Assumptions** (change them and the totals move, not the ranking):

- every deposit stays in store for 9 months;
- 30% of deposits are pledged, on day 30, for 90 days, then released; the
  lot exits at the end;
- recoveries are rare and left out;
- about 50 deposits per warehouse per month (4 warehouses at 200 a month, 40
  at 2,000), which sets how many daily anchors are shared per deposit.

Because anchors scale with warehouses, and warehouses with deposits, the
per-deposit cost is the same at 200 and at 2,000 a month. A warehouse with
fewer deposits pays more per deposit for its anchor (see B).

## Cost comparison

| design | rate | XLM per deposit | vs current | 200 deposits / month | 2,000 deposits / month |
|---|---|---:|---:|---:|---:|
| Current (`data_version` 2) | Testnet | 1.4343 | +0% | 286.9 | 2,869 |
| Current (`data_version` 2) | mainnet today | 0.2734 | +0% | 54.7 | 547 |
| Current (`data_version` 2) | mainnet at 10,000/KB | 2.6669 | +0% | 533.4 | 5,334 |
| A: mint on demand | Testnet | 0.3717 | -74% | 74.3 | 743 |
| A: mint on demand | mainnet today | 0.0715 | -74% | 14.3 | 143 |
| A: mint on demand | mainnet at 10,000/KB | 0.6905 | -74% | 138.1 | 1,381 |
| A+B: + daily anchoring | Testnet | 0.3876 | -73% | 77.5 | 775 |
| A+B: + daily anchoring | mainnet today | 0.0751 | -73% | 15.0 | 150 |
| A+B: + daily anchoring | mainnet at 10,000/KB | 0.7194 | -73% | 143.9 | 1,439 |
| A+B+C: + aggregated lots (3 per entry) | Testnet | 0.3294 | -77% | 65.9 | 659 |
| A+B+C: + aggregated lots (3 per entry) | mainnet today | 0.0643 | -76% | 12.9 | 129 |
| A+B+C: + aggregated lots (3 per entry) | mainnet at 10,000/KB | 0.6109 | -77% | 122.2 | 1,222 |
| D: minimal chain | Testnet | 0.1720 | -88% | 34.4 | 344 |
| D: minimal chain | mainnet today | 0.0339 | -88% | 6.8 | 68 |
| D: minimal chain | mainnet at 10,000/KB | 0.3186 | -88% | 63.7 | 637 |

The C row assumes 3 lots per aggregate entry. Its saving at 1, 3 and 10
lots is under C below.

## Current design

**Stores:** every deposit mints a receipt (432 bytes) and a deposit guard
(124 bytes), plus index entries, all kept alive for the deposit's life.
Every partial operation, lock, recovery and sale is on-chain.

### Verification

- **A lender can verify independently** that the pledged receipt exists,
  who attested it (a registered custodian and verifier), its weight, grade
  and owner, that it is locked to them and to no one else, that it cannot
  be exited, transferred or re-pledged while locked, and the full lineage of
  every split. The contract enforces all of this. No call to Farmledge is
  needed: any RPC node will do.
- **On trust:** that the grain physically exists and matches the receipt
  (true of every design: the chain records attestations, it does not weigh
  grain), and, in Phase 1, that the custodian and verifier signatures mean
  anything, since Farmledge holds those keys.
- **A farmer can prove** to anyone that a receipt for their deposit exists
  and is theirs, from the chain alone.
- **If Farmledge disappeared tomorrow:** every receipt, lock, sale and
  settlement survives on-chain, readable by anyone (archived entries are
  restorable by whoever pays). Lost: the mapping of addresses to people,
  and, while Farmledge holds farmer keys, the farmers' ability to sign
  anything.

## A: mint on demand

**Stores:** nothing for a deposit until it is first needed (pledged, or
requested by the farmer or a buyer). Then a receipt like today's, minus the
deposit guard. About 70% of deposits never touch the chain.

**Measured:** a mint on demand costs 6.0M stroops on Testnet (0.115 XLM on
mainnet today), against 7.6M for today's mint with its guard. The saving is
almost entirely the 70% that are never minted.

**What breaks, and the fixes:**

- **Duplicate protection.** Today the `DepositIndex` guard is one 124-byte
  entry per deposit. With most deposits off-chain, a per-deposit entry is
  exactly the cost we are removing. **Fix:** key deposits by a per-warehouse
  sequence number from the database, and keep one bit per number in a
  bitmap: one 252-byte entry covers 1,024 deposits (measured: 3,200 stroops
  per deposit on Testnet, amortised). A second mint of the same number
  fails. The bitmap does not need keeping alive: like today's guard, an
  archived chunk is restored when a mint reads it, so the protection
  survives archival. **What this does not stop** (nor does today's guard):
  Farmledge assigning two numbers to one physical lot. B narrows that, see
  A+B.
- **Lineage.** Unchanged for anything minted: splits, sales and history
  work as today from the moment of minting. Before minting, the lot's
  history (intake, partial dispatches) is in the database only. A minted
  receipt starts from the lot's *current* balance, so its on-chain lineage
  begins at the pledge.
- **On-chain supply matches physical stock.** Broken, deliberately. The sum
  of on-chain receipts becomes the pledged (or requested) subset, not the
  warehouse's stock. An auditor can no longer reconcile a warehouse's
  physical count against the chain. B restores a commitment to the full
  stock.

### Verification

- **A lender can verify independently** everything it can today about the
  pledged lot, from the moment it is minted: attesters, weight, lock,
  lineage, recovery and sale.
- **On trust:** that the minted receipt reflects a real deposit with that
  history. The receipt appears on-chain only when the pledge does, with
  nothing earlier to check it against.
- **A farmer can prove** an unpledged deposit to no one but Farmledge,
  unless they ask for a receipt to be minted (which costs a mint).
- **If Farmledge disappeared tomorrow:** pledged receipts survive as today.
  **Unpledged deposits are lost**: they existed only in the database.

## B: daily anchoring

**Stores:** per warehouse, one hash-chained **head** (160 bytes), updated
daily, plus that day's Merkle **root** (212 bytes). The head is
`sha256(previous head ‖ day ‖ root)`, so the latest head commits to every
root before it. Each anchor also emits an `anchored` event carrying the
day, root and head.

### Leaf format

One tree per warehouse per day, over three kinds of leaf:

| kind | one leaf per | meaning |
|---|---|---|
| 0, deposit | intake that day | a lot arrived |
| 1, dispatch | dispatch that day | grain left, from which lot |
| 2, balance | active lot at end of day | what the lot still holds |

Each leaf is this record:

| field | type | |
|---|---|---|
| version | u32 | 1 |
| kind | u32 | 0, 1 or 2 |
| warehouse_id | Symbol | |
| day | u32 | days since 1970-01-01, UTC |
| index | u32 | the leaf's position in the tree: a position, **never an identity** |
| deposit_no | u64 | the lot's per-warehouse number, assigned once by the database: **the lot's identity, and the key of the duplicate guard** |
| deposit_id | String (≤ 64 bytes) | the lot's human reference |
| receipt_id | u64 | the on-chain receipt that now governs the lot, or 0 if none |
| receipt_version | u32 | which contract `receipt_id` belongs to: 2 (the current contract, during cutover) or 3; 0 if none |
| owner | Address | |
| commodity, grade | Symbol | |
| kg | u64 | deposited, dispatched or remaining |
| bags, moisture_bps | u32 | |
| custodian, verifier | Address | who attested this entry |
| at | u64 | Unix seconds of the event (end of day for balances) |
| salt | BytesN<32> | random per leaf, held by Farmledge and given to the farmer |

- **leaf hash** = `sha256(0x00 ‖ XDR(leaf))`, where `XDR(leaf)` is the
  record encoded as the Soroban `ScVal` the contract's own `leaf_hash` read
  function produces.
- **node** = `sha256(0x01 ‖ left ‖ right)`. The 0x00/0x01 prefixes stop a
  leaf being passed off as a node.
- Leaves are ordered by (kind, deposit_id), and the tree is padded to a
  power of two with `sha256(0x00)`.

**The salt matters.** Leaf contents are guessable (owners are public
addresses, weights are round numbers). Without a salt, anyone holding a leaf
hash could confirm a guess about a farmer's holdings. With one, only
holders of the salt can link a leaf to a lot.

### How a third party verifies that a deposit was included

They need the leaf's fields and salt, and its proof (the sibling hashes up
the tree, plus the index). The farmer can hand these over directly, so
Farmledge is not involved. Then:

1. **Compute the leaf hash.** Use any Stellar XDR library, or simulate the
   contract's `leaf_hash` (a free read: no signature, no fee).
2. **Fold the proof into a root.** At each level, hash with the sibling on
   the side the index bit says.
3. **Check the root against the chain:**
   - **Within 30 days:** compare it with the day root the contract stores.
     The contract can do this check itself; `b_verify` does.
   - **Older:** take every `anchored` event for that warehouse from that day
     onwards, and recompute the head forward from that day's root. If the
     result equals the head stored on-chain now, every root in the sequence
     is authentic. The events can come from any archive, even Farmledge's:
     they are checked against the chain, not trusted.

That proves the deposit, dispatch or balance was in the warehouse's
committed books for that day, and that Farmledge cannot have rewritten that
day since. It does **not** prove the books were complete or honest. An
auditor holding the full leaf set for a day can check its totals, and
compare them with a physical count.

### Cost, and how long anchors must live

| | Testnet | mainnet today | mainnet at 10,000/KB |
|---|---:|---:|---:|
| one daily anchor (head + 30-day temporary day root) | 0.0249 | 0.0057 | 0.0454 |
| head kept alive, per year | 0.4362 | 0.0821 | 0.8121 |
| **per warehouse per year** | **9.54** | **2.15** | **17.39** |
| for contrast: day roots kept on-chain for a year instead | 210.91 | 39.68 | 392.72 |

Keeping the day root on-chain for a whole year instead would cost 18–23×
more. The
recommended split:

- **Head: kept alive indefinitely.** It costs 0.08 XLM a year on mainnet
  today. If it ever archives, nothing is lost: it is restored automatically
  by the next anchor, or by anyone who reads it.
- **Day root: temporary storage, 30 days.** It exists so that a mint or
  pledge can prove a lot against a recent day *on-chain*. After 30 days the
  network deletes it (temporary entries are not restorable). **What is lost
  then:** only the ability to check that day's proofs on-chain. Off-chain
  verification still works through the head chain. The roots stay in the
  `anchored` events, which the indexer archives (a correctness requirement,
  see `docs/events.md`), and which can always be recovered by replaying
  Stellar ledger history.

## A+B: mint on demand, anchored daily

**Stores:** B for every warehouse, plus A's receipts for pledged lots.
**The mint on demand should require an inclusion proof** of the lot's latest
balance leaf against a stored day root. That binds every receipt to the
committed books. It also stops a receipt being minted for a lot the books
do not show, for more than the books show, or twice (with A's bitmap). The
proof check is cheap: at depth 11 (2,048 leaves, a warehouse with about
2,000 active lots) it measured 134,000 instructions in the test host. Even
several times that once compiled to Wasm, it costs well under 0.0001 XLM.
Each extra level of tree adds one hash.

### Verification

- **A lender can verify independently** everything it can today about the
  pledged lot. In addition, it can check that the lot was in the warehouse's
  committed balance on the day it was pledged, and on every later day, if
  the farmer or Farmledge supplies that day's balance proof.
- **On trust:** the completeness of each day's tree, which an auditor with
  the full leaf set can check. And, as everywhere, the physical grain and
  the Phase 1 signatures.
- **A farmer can prove** to anyone that their deposit was in a warehouse's
  committed books on a given day, using their leaf, salt and proof against
  the chain. That needs no minting and no Farmledge, **provided Farmledge
  has handed them the proof** (an SMS link or a downloadable receipt at
  every deposit and dispatch). Without that hand-over, the proof is in
  Farmledge's database only.
- **If Farmledge disappeared tomorrow:** pledged receipts survive as today.
  The anchors survive and prove which roots were committed. Unpledged
  deposits survive **only as proofs that farmers hold.** The leaf data is
  not on-chain, so a farmer without their proof has nothing to show.

## C: aggregated entries

**Stores:** one entry per (farmer, warehouse, commodity) holding all of that
farmer's active lots, instead of one receipt (plus guard) per lot.

**Measured,** per lot, including its share of the entry's keep-alive:

| lots in the entry | entry bytes | bytes per lot | C: per lot, mainnet today | vs A receipt (pledged deposit) | vs current receipt + guard | C per lot at 10,000/KB |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 372 | 372 | 0.1687 | -17% | -35% | 1.6639 |
| 3 | 764 | 254 | 0.1160 | -43% | -55% | 1.1395 |
| 10 | 2,136 | 213 | 0.0976 | -52% | -62% | 0.9562 |

The first column of savings compares with A's receipt (a pledged deposit
under A); the second compares with today's receipt plus guard. Each extra
lot adds about 196 bytes, against 432 for a receipt. Under A only pledged
lots are on-chain, and few farmers have many pledged lots at once. So C's
saving over A+B is about 0.011 XLM per deposit today (0.11 at 10,000/KB).

**Concurrency and size limits:**

- **One key per farmer, warehouse and commodity.** Every intake, pledge,
  release and exit for that farmer's lots writes the same entry. Soroban
  orders transactions that write the same key, so they never corrupt each
  other. But a transaction is simulated against the entry's current size,
  and if another write grows the entry first, the later one can exceed its
  declared write bytes and fail. **The backend must serialise writes per
  farmer** (or retry), and a lender's pledge can collide with the
  custodian's intake.
- **The whole entry is rewritten on every operation.** Write cost, and the
  rent on any growth, grow linearly with the lots in it. The measured pledge
  of one lot among 10 wrote 2,636 bytes. A receipt-per-lot pledge writes
  about 940.
- **Hard ceiling: 64 KB per entry,** about 330 lots at 196 bytes each.
  Long before that, every operation is paying to rewrite kilobytes. Exited
  lots must be removed to keep the entry small, and then their on-chain
  record is gone and only the events remain.
- **Keep-alive is all-or-nothing.** One active lot keeps every lot in the
  entry alive.
- **Lineage gets harder.** Partial pledges and splits become edits inside
  one entry, with no per-lot id that a lender can hold.

### Verification

- **A lender can verify independently** that the lot is in the farmer's
  aggregate and marked as pledged to them. It can also check everything the
  lot record holds.
- **On trust:** the same as A+B. Also, a lender's view of "its" lot is a row
  inside an entry that other operations keep rewriting, so the contract
  code must guarantee that no operation on one lot can touch another.
  Today that guarantee is structural: each lot is a separate entry.
- **A farmer can prove** what A+B allows, and can show all of one
  commodity's on-chain lots in a single read.
- **If Farmledge disappeared tomorrow:** the same as A+B.

## D: minimal chain

**Stores:** B's anchors, and from the pledge onwards only the lending
lifecycle: one pledge record (380 bytes) per pledged lot, created from the
database record together with its inclusion proof. No receipts, no
exits, no splits on-chain.

**Measured:** a pledge costs 5.2M stroops on Testnet (0.10 XLM on mainnet
today), and a release 6,900 stroops. That is the floor: about 88% below
the current design.

**A flaw the prototype found, with its fix.** The prototype guarded against
double pledges by leaf (warehouse, day, index). But a lot gets a new balance
leaf every day, so the same lot could be pledged twice using two different
days' leaves. The guard must be keyed by deposit number (A's bitmap), not
by leaf. The cost difference is negligible.

### Verification

- **A lender can verify independently** that, on the pledge day, the lot was
  in the warehouse's committed balance with the stated weight, owner and
  attesters, and that the lot is not pledged to anyone else. It can also
  verify the pledge's own lifecycle: release, recovery, settlement.
- **On trust:** everything that happens to the grain after the pledge.
  Nothing on-chain stops a dispatch of a pledged lot. Today a locked
  receipt cannot be exited on-chain. Under D the lender can only *detect*
  a dispatch afterwards, if it obtains later balance proofs, or a proof
  that the lot is absent, which needs the full leaf set from Farmledge.
  Partial releases, sales to a buyer and the buyer's later exit become
  database records, attested only by the next day's anchor.
- **A farmer can prove** inclusion, as in A+B (with proofs they hold). On
  the chain itself they own nothing, only a pledge record that names them.
- **If Farmledge disappeared tomorrow:** the anchors and the pledge records
  survive. Everything else, including who owns what and who bought a lot
  in recovery, survives only as proofs in farmers' and lenders' hands.

## Audit of A+B: what every check is keyed on

Written before any A+B code, after the D prototype turned out to let one lot
be pledged twice through two days' leaves. The question for every check:
**does it rest on a stable identifier, or on something that changes over
time** (a daily leaf, a root, a day number, a timestamp, a caller-supplied
value nothing binds)? Every check is listed, sound or not.

The rule that comes out of it: **identity and quantity come only from the
proven leaf, and uniqueness is keyed only on the lot's stable identity
`(warehouse_id, deposit_no)`, never on a leaf hash, leaf index, day or
root.**

### Minting on demand: the deposit guard and the proof

| # | check | keyed on | stable? | verdict |
|---|---|---|---|---|
| A1 | **Deposit guard under A**: a lot can be minted once | prototype: `warehouse_id` and `deposit_no` passed as **separate arguments**, unrelated to anything proven | the number is stable, but **nothing binds it to the lot** | **Flaw, D's class.** Whoever holds the custodian and verifier keys (Farmledge in Phase 1) could mint the same proven lot twice under two numbers. Unlike D, the key is not time-varying; but it is not the lot's identity either. **Fix:** the guard key is `(leaf.warehouse_id, leaf.deposit_no)`, read from the proven leaf, with no separate arguments. The same lot then maps to the same bit on every day, through every leaf, root and index. |
| A2 | One physical lot has one `deposit_no` | the database's sequence | stable, but off-chain | **Sound, with stated trust.** The contract can only enforce "each number mints once". That a lot was not given two numbers at intake is the backend's guarantee, as today's client-chosen `deposit_id` is. It is checkable: each day's tree holds one deposit leaf per number, and the numbers are contiguous. |
| A3 | The guard survives time | persistent bitmap entries | stable | **Sound, and must stay so.** An archived chunk is restored when a mint reads it, so the guard still holds. It must **never** move to temporary storage: expiry would delete it and reopen every number in the chunk. A test pins this. |
| A4 | Which chunk a number lives in | `deposit_no / 1024` | stable | Sound. |
| A5 | Which kind of leaf may mint | prototype: **any kind** | **time-varying.** A deposit leaf (kind 0) records the intake weight, which later dispatches reduce | **Flaw.** A lot deposited at 2,000 kg and since drawn down to 500 could be minted at 2,000 from its old deposit leaf. **Fix:** only balance leaves (kind 2) can mint. |
| A6 | How recent the leaf must be | prototype: **any day whose root is still stored** (30 days) | **time-varying** | **Flaw.** A balance leaf from before a dispatch overstates what the lot holds now. **Fix:** `leaf.day` must equal the warehouse's latest anchored day. The window that is left (dispatches since that anchor, at most a day) is closed by procedure: a mint is co-signed by a custodian and verifier attesting the lot as it is now, and the backend freezes off-chain dispatch of a lot with a mint pending. |
| A7 | No uniqueness check uses a leaf, index, day or root | — | — | **Holds once A1 is fixed.** This is exactly D's bug: D keyed its guard on (warehouse, day, leaf index). |
| A8 | The receipt's contents | prototype: owner, warehouse, commodity, grade, kg, bags, moisture **passed as arguments**, with no proof at all | caller-supplied | **Flaw.** With a proof added but fields still passed separately, the proof would check one leaf while the receipt was written from other values. **Fix:** every field of the minted receipt is taken from the proven leaf, including its original custodian and verifier, which record who attested the intake. The only arguments are the leaf, the proof and the signers at mint time. |
| A9 | Which root a proof is checked against | `(leaf.warehouse_id, leaf.day)`, from the leaf itself | stable once A6 fixes the day | Sound. The position comes from `leaf.index`, which is only ever a position. |
| A10 | A node cannot pose as a leaf | domain separation: `0x00` for leaves, `0x01` for nodes | — | **Sound.** Defence in depth: also require the proof's length to match the depth implied by that day's stored leaf count. |

### Anchoring

| # | check | keyed on | stable? | verdict |
|---|---|---|---|---|
| A11 | Days are anchored in order | prototype: rejects a day ≤ the last one, **except** when the last one is day 0, and has **no upper bound** | time, uncontrolled | **Flaw.** Day 0 can be re-anchored repeatedly. A far-future day can be anchored, after which no real day can be, and A6 would point every mint at the fake day. **Fix:** track "no anchor yet" explicitly (not as day 0), and require `last day < day ≤ the ledger's current day` (ledger timestamp ÷ 86,400). Ledger time is consensus time, the right clock. |
| A12 | A day's root cannot be replaced | strict ordering (A11) | stable | **Sound once A11 is fixed.** Replacing a root would keep proofs alive for lots since dispatched, or rewrite the books. |
| A13 | Who may anchor | a custodian registered for that warehouse, now | the registry | **Sound.** Removing a custodian stops their future anchors; past anchors stand. Trust: in Phase 1 the custodian key is Farmledge's, so an anchor is Farmledge's statement. What the chain adds is that it **cannot be rewritten later**. |
| A14 | Ordering survives time | the head is a persistent entry | stable | **Sound, and must stay so.** An archived head is restored on read, so ordering holds. In temporary storage, expiry would reset ordering and allow old days to be re-anchored. |
| A15 | The day root expiring | temporary storage, 30 days | time-varying, deliberately | **Sound.** Only which day can be proven on-chain depends on it. With A6, mints only ever use the latest root. No uniqueness depends on it. |
| A16 | The hash chain commits to the day | `head = sha256(previous head ‖ day ‖ root)` | stable | Sound. |

### After mint, and across the cutover

| # | check | keyed on | stable? | verdict |
|---|---|---|---|---|
| A17 | Lock, unlock, extend_due, recovery, settlement, exit, revoke | the receipt id; the lender recorded in the lock; registries | stable | **Sound.** Unchanged from `data_version` 2 and covered by `docs/adversarial-review.md`. |
| A18 | No double pledge of a minted receipt | receipt status (`Active` only) and receipt id | stable | **Sound.** The only way to pledge one lot twice is a second receipt for it, which A1, A5, A6 and A19 close. |
| A19 | A lot already held as a current-contract receipt | **nothing**: v3's bitmap does not know the current contract's `DepositIndex` (different ids) | — | **Flaw at cutover.** Those lots appear in v3's first balance leaves, so the same grain could be minted on v3 and pledged on both contracts. **Fix:** before v3 accepts any mint, the admin pre-claims those lots' deposit numbers in v3's bitmap (an on-chain, checkable list), and their balance leaves carry the current-contract receipt id in `receipt_id`. |
| A20 | A minted lot's later life stays on-chain | backend routing | procedure | **Not a contract check, and not preventable on-chain.** Once minted, the receipt governs the lot, and its dispatches must go through `exit`. Its balance leaves keep appearing, with `receipt_id` set, so auditors don't count it twice. If the backend dispatched it off-chain, the anchors would show less than the receipt: detectable, so it must raise an alert. |
| A21 | Due dates and recovery timing | the ledger timestamp against stored dates | consensus time | Sound. |

**Result:** six flaws (A1, A5, A6, A8, A11, A19), all in the prototype's
shape or the cutover, none in the current contract. Three (A1, A5, A6) are
D's class: a lot's identity or quantity taken from something unbound or
time-varying instead of the proven leaf. Each fix becomes a named regression
test before the code that satisfies it. The other fifteen checks are sound
as designed, three of them (A3, A14, A20) only as long as the stated rule is
kept.

## Farmer proof delivery: a requirement

Every deposit and dispatch hands the farmer their leaf, salt and inclusion
proof **automatically**, not on request. Without that, unpledged inventory
is provable only through Farmledge, which defeats the point of anchoring.
The bundle format and how a third party verifies one with nothing but the
bundle and the chain are in `docs/events.md` ("Farmer proofs"); delivery
obligations and alerts are in `docs/backend-requirements.md`.

## Recommendation

**Build A+B.** It means:

- daily per-warehouse anchors: a hash-chained head kept indefinitely, plus
  the day's root in temporary storage for 30 days;
- receipts minted on demand, for pledges or on request, each proving the
  lot against a recent anchored balance;
- duplicate protection from a per-warehouse bitmap of deposit numbers;
- the current receipt lifecycle and compact layout, unchanged, for
  everything that is minted.

Why:

- **It captures most of the saving:** 73% off per deposit (0.273 → 0.075
  XLM today, 2.667 → 0.719 at 10,000/KB). At 2,000 deposits a month that is
  547 → 150 XLM a month today, or 5,334 → 1,439 at 10,000/KB.
- **It keeps enforcement where trust is needed.** A pledged lot is a real
  receipt that the contract locks, splits, sells and settles. That is what
  a lender needs from a chain it can check without trusting us.
- **Unpledged inventory becomes provable without being on-chain,**
  through anchors. Today it is on-chain but expensive.

**Not C.** Under A+B it saves about 0.011 XLM a deposit, at the price of
per-farmer write contention, entries that grow, and lot isolation enforced
by code rather than by storage. Revisit it only if many farmers routinely
hold several pledged lots at once.

**D, only if A+B is still too expensive at 10,000/KB.** D saves a further
0.04 XLM per deposit today, 0.40 at 10,000/KB, but it turns the lender's
on-chain *enforcement* into after-the-fact *detection*. The figure to weigh
is 0.40 XLM per deposit at the planning rate against revenue per deposit.
If revenue cannot carry A+B at that rate, D is the honest fallback, and the
lender-facing terms should then say plainly that collateral custody after
the pledge rests on Farmledge and the warehouse, not on the chain.

**The farmer must hold their own proofs.** This is a requirement, not an
option. See [Farmer proof delivery](#farmer-proof-delivery-a-requirement).

## Implementation plan for A+B (for review)

**Not started.** Nothing below is implemented until this plan is approved.

### Contract: `data_version` 3

A new deployment built from the current code. The current contract is
immutable and keeps running.

1. **Anchoring.** `anchor(custodian, warehouse_id, day, root, leaves)`:
   - a custodian registered for the warehouse signs (A13);
   - `last day < day ≤ ledger day`, with "never anchored" tracked explicitly
     rather than as day 0 (A11, A12);
   - the head goes in persistent storage and the day root in temporary
     storage for 30 days (A14, A15);
   - `head = sha256(prev ‖ day ‖ root)` (A16), and it emits `anchored`.
2. **Mint on demand.** `mint_from_anchor(custodian, verifier, leaf, proof)`
   replaces `mint`:
   - the leaf must be a balance leaf (A5) from the warehouse's latest
     anchored day (A6);
   - the proof's length matches the day's depth, and it folds to that day's
     root (A9, A10);
   - the guard claims the bit for `(leaf.warehouse_id, leaf.deposit_no)` in a
     persistent bitmap (A1, A3, A4, A7);
   - every receipt field is copied from the leaf (A8);
   - a custodian and verifier sign at mint time.
3. **Pre-claim for cutover** (A19). `preclaim(warehouse_id, deposit_nos)`:
   admin only, at most 25 numbers per call, and it fails on any number
   already claimed. It emits one event per number, so the list is public and
   checkable.
4. **Reads:** `leaf_hash(leaf)` and `verify_leaf(leaf, proof)`, free to
   simulate, for verifiers who do not want to implement the hashing.
5. **Removed:** `mint` and the `DepositIndex` guard (`extend_ttl_batch` no
   longer bumps it). The bitmap is never kept alive, because an archived
   chunk is restored when it is read.
6. **Receipt layout:** gains `deposit_no`, the link back to the anchored
   leaves. `DATA_VERSION` becomes 3. Everything else is unchanged: lock,
   unlock, extend_due, recovery, settlement, exit, revoke, in-place splits,
   the compact layout, the 64-byte caps, the keep-alive skip.
7. **New errors:** `WrongLeafKind`, `StaleLeaf`, `InvalidProof`, `NoAnchor`,
   `AnchorOutOfOrder`, `AnchorInFuture`. A repeat claim keeps
   `DuplicateDepositId`.
8. **Tests first:**
   - one named regression test per audit finding;
   - a stateful property test over random anchors, mints, exits and
     pledges: never two receipts for one `(warehouse_id, deposit_no)`, never
     a mint above the latest anchored balance, anchors always strictly
     ordered and never ahead of the ledger;
   - mutation testing over the new code.
9. **Then:** deploy to Testnet, extend `scripts/measure_costs.py` to cover
   anchors and minting on demand, and confirm the costs in this document
   with v3's own fees. **An external audit before mainnet:** the proof
   check, the guard and the anchor ordering are new security-critical code.

### Backend

All of it is in `docs/backend-requirements.md` (items 3–6):

- **Postgres** becomes the system of record for unpledged lots: deposits
  with per-warehouse `deposit_no` sequences, dispatches, end-of-day
  balances, and every leaf, salt, tree and proof.
- **The anchoring job,** daily per warehouse. It confirms each anchor
  on-chain and alerts on a missed cutoff.
- **Proof delivery:** a QR code on the counter slip; the SMS bundle within
  24 hours of the anchor; monthly balance statements; a static verifier page
  that checks proofs against a public RPC; delivery receipts and alerts. An
  open-source standalone verifier is published with the bundle spec.
- **Pledge flow:** freeze off-chain dispatch of the lot, mint from its
  latest balance proof, then lock. Once minted, the lot only exits
  on-chain. An alert fires if its anchored balance and the receipt ever
  disagree.
- **Indexer and keep-alive:** the indexer covers the new events; the
  keep-alive job covers both contracts.

### Cutover, by date

Mainnet has not launched, so **mainnet starts directly on `data_version`
3**. The cutover below is for Testnet and pilot data.

1. **Shadow period, at least two weeks.** v3 runs on Testnet alongside the
   current contract, anchoring daily with no mints, while deposits still
   mint on the current contract. The anchoring job, proof delivery and the
   verifier are exercised end to end. Exit criteria: no missed anchors,
   every bundle delivered within 24 hours, and every bundle verified by the
   standalone verifier.
2. **Choose cutover day X:** a 00:00 UTC cutoff after the shadow period
   meets its criteria, announced in advance.
3. **On X − 1:**
   - stop minting on the current contract at the cutoff;
   - pre-claim, on v3, the deposit numbers of every lot still held as a
     current-contract receipt;
   - publish that list.
4. **From X:**
   - new deposits are database-only and anchored daily;
   - a pledge mints on v3;
   - lots held as current-contract receipts stay there until they exit,
     and appear in v3's balance leaves with `receipt_version` 2;
   - the keep-alive job covers both contracts.
5. **When the last current-contract receipt exits,** mark that contract
   retired, as earlier deployments were.

## Migration path

The current contract is immutable (`docs/upgrade.md`), so this is a new
deployment alongside it, not an upgrade.

1. **Build `data_version` 3** from the current code:
   - add anchoring (head + temporary day root), and mint-on-demand that
     requires an inclusion proof against an anchored balance leaf;
   - replace the per-deposit `DepositIndex` with the deposit-number bitmap;
   - drop the plain `mint`;
   - keep everything else: lock, unlock, recovery, settlement, exit,
     in-place splits, the compact layout, the ref caps.
2. **Backend:**
   - Postgres becomes the system of record for unpledged lots, with
     per-warehouse deposit numbers;
   - a daily job builds each warehouse's tree (deposits, dispatches,
     balances) and calls the anchor;
   - every leaf, salt and proof is stored, and handed to the farmer;
   - the indexer's correctness requirements (`docs/events.md`) extend to
     `anchored` events.
3. **Testnet:** deploy v3, rerun `scripts/measure_costs.py` extended for
   anchors and minting on demand, and confirm the figures above with v3's
   own fees. Then an external audit before mainnet: the proof check and the
   bitmap guard are new security-critical code.
4. **Cutover by date, no state migration.** From day X, new deposits are
   database-only and anchored, and are minted on v3 when pledged. Receipts
   already on the current contract stay there and live out their lifecycle,
   because they are already paid for. The keep-alive job runs for both
   contracts until the last current-contract receipt exits. Then the current
   contract is marked retired and left to archive.
5. **Reconcile once:** before v3 accepts a mint, the deposit numbers of lots
   still held as current-contract receipts are pre-claimed on v3 (audit
   finding A19), and v3's first anchor of each warehouse includes their
   balance leaves with `receipt_version` 2. That puts the whole stock under
   one daily commitment from day one, without any lot being mintable twice.

Nothing in this path touches existing receipts or their owners, and nothing
needs rolling back: if v3 is abandoned, the current contract is still
running.

## Appendix: prototype measurements

Testnet, prototype contract `CBSD3ZLE…RSB6S5`, one run on 2026-10-04. The
first anchor and the first pledge also create their warehouse's head or
bitmap; the "later" rows are the steady-state cost.

| prototype operation | Testnet fee (stroops) | of which rent | mainnet today (XLM) | mainnet at 10,000/KB (XLM) | CPU instr. | write B | tx |
|---|---:|---:|---:|---:|---:|---:|---|
| A a_mint — first in its bitmap chunk (creates owner index) | 9,295,920 | 9,258,947 | 0.1763 | 1.7295 | 2,316,377 | 1,604 | `8c0f791ba671` |
| A a_mint — later deposit, chunk exists | 6,000,616 | 5,963,604 | 0.1149 | 1.1153 | 2,341,662 | 1,628 | `b9e0059f00d6` |
| B b_anchor — first day, head + day root stored | 4,878,850 | 4,865,886 | 0.0920 | 0.9083 | 991,373 | 372 | `563eca2c6505` |
| B b_anchor — next day, head + day root stored | 2,785,655 | 2,772,677 | 0.0530 | 0.5181 | 1,010,766 | 372 | `053933fd8e5f` |
| B b_anchor — head only (day root in the event) | 9,806 | 0 | 0.0010 | 0.0010 | 930,770 | 160 | `8cea5f9b77d1` |
| D d_pledge — proof depth 11, first pledge (creates leaf bitmap, lender index) | 12,913,428 | 12,870,418 | 0.2442 | 2.4033 | 2,400,087 | 1,288 | `2daae8c922a3` |
| D d_pledge — proof depth 11, later pledge | 5,170,417 | 5,127,337 | 0.0999 | 0.9600 | 2,486,589 | 1,300 | `b0ff0686e694` |
| D d_release | 6,929 | 0 | 0.0007 | 0.0007 | 827,012 | 380 | `979e750d7d36` |
| C c_add_lot — lot 1 into the farmer's entry | 8,181,683 | 8,161,347 | 0.1542 | 1.5233 | 1,572,494 | 700 | `1117f79b39fb` |
| C c_add_lot — lot 2 into the farmer's entry | 2,584,273 | 2,563,726 | 0.0498 | 0.4799 | 1,634,727 | 896 | `41ed6f8a42ad` |
| C c_add_lot — lot 3 into the farmer's entry | 2,584,451 | 2,563,722 | 0.0499 | 0.4799 | 1,655,674 | 1,092 | `b08d43201da7` |
| C c_add_lot — lot 4 into the farmer's entry | 2,584,629 | 2,563,718 | 0.0499 | 0.4800 | 1,676,620 | 1,288 | `866456710990` |
| C c_add_lot — lot 5 into the farmer's entry | 2,584,809 | 2,563,715 | 0.0499 | 0.4800 | 1,697,568 | 1,484 | `661e1640fe25` |
| C c_add_lot — lot 6 into the farmer's entry | 2,585,939 | 2,564,664 | 0.0499 | 0.4802 | 1,718,513 | 1,680 | `62c27620e848` |
| C c_add_lot — lot 7 into the farmer's entry | 2,586,118 | 2,564,660 | 0.0499 | 0.4802 | 1,739,461 | 1,876 | `4d3e29497e26` |
| C c_add_lot — lot 8 into the farmer's entry | 2,586,296 | 2,564,656 | 0.0500 | 0.4802 | 1,760,406 | 2,072 | `d8edb36e6670` |
| C c_add_lot — lot 9 into the farmer's entry | 2,586,472 | 2,564,651 | 0.0500 | 0.4802 | 1,781,354 | 2,268 | `b7e5c5c43fdf` |
| C c_add_lot — lot 10 into the farmer's entry | 2,586,652 | 2,564,648 | 0.0500 | 0.4802 | 1,802,300 | 2,464 | `65a5a3ce2a41` |
| C c_pledge_lot — one lot of 10 | 1,438,461 | 1,414,364 | 0.0288 | 0.2660 | 1,911,168 | 2,636 | `d74c01c7a17d` |

Entry sizes, read back with `getLedgerEntries`:

| entry | bytes |
|---|---:|
| A receipt | 432 |
| A bitmap chunk (1,024 deposits) | 252 |
| B day root | 212 |
| B head | 160 |
| D pledge record | 380 |
| D leaf bitmap (1,024 leaves) | 256 |
| C aggregate entry, 1 lot(s) | 372 |
| C aggregate entry, 3 lot(s) | 764 |
| C aggregate entry, 10 lot(s) | 2,136 |
| C aggregate entry, 10 lots, one pledged | 2,232 |
