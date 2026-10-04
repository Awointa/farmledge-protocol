# Backend requirements

What the protocol needs from the Farmledge backend to be correct. These are
**correctness requirements**: missing one loses data or verifiability, and
is treated as a bug with the same priority as a contract bug.

| | applies | detail |
|---|---|---|
| 1. Indexer: archive every event durably, alert when a day behind, check for gaps after outages, back up | now | `docs/events.md` ("Indexer requirements") |
| 2. Keep-alive job: daily, 175-day bump, alert at 14 days left, never send `Exited` or `Revoked` ids | now | `docs/archival.md` ("The keep-alive job") |
| 3. Anchoring job | from `data_version` 3 | below |
| 4. Farmer proof delivery | from `data_version` 3 | below |
| 5. Minted lots are governed on-chain | from `data_version` 3 | below |
| 6. Cutover | once, at the switch to `data_version` 3 | `docs/storage-strategy.md` ("Migration path") |

Items 3–6 belong to the A+B design in `docs/storage-strategy.md` and are
**not implemented yet**.

## 3. Anchoring job (planned)

- **Once a day per warehouse,** at a fixed UTC cutoff, build that day's tree
  from Postgres: one deposit leaf per intake, one dispatch leaf per
  dispatch, one balance leaf per active lot, in the leaf format of
  `docs/storage-strategy.md`. That includes lots governed by an on-chain
  receipt, with `receipt_id` set.
- **Every leaf gets a fresh random 32-byte salt,** stored with the leaf and
  never reused.
- **Submit the anchor** signed by a custodian registered for the warehouse,
  and confirm it on-chain: the stored head equals the one computed locally.
  Only then is the day anchored.
- **Persist every leaf, salt, proof and root** before delivering anything.
  They fall under the indexer's archive and backup rules: the leaf data is
  stored nowhere else.
- **Alert** if any warehouse's day is not anchored within 2 hours of the
  cutoff. Days must be anchored in order and cannot be anchored later out of
  order, so a missed day is anchored late, never skipped.
- **Deposit numbers** are a per-warehouse sequence. They are assigned once,
  at intake, and are never reused or reassigned. One physical lot has
  exactly one number. Each day's tree must hold one deposit leaf per new
  number, with no gaps. A reconciliation check enforces both.

## 4. Farmer proof delivery (planned)

Automatic, never on request. For every deposit and every dispatch:

1. **At the counter,** the printed slip carries a QR code with the leaf's
   data and salt (the proof cannot exist yet, because the day's tree is
   built at the cutoff).
2. **Within 24 hours of the cutoff,** once the anchor is confirmed on-chain,
   the farmer receives the full proof bundle (format in `docs/events.md`,
   "Farmer proofs"):
   - by SMS, as a link to a page that shows the bundle as a QR code and a
     file, needs no login, and checks the proof **in the browser against a
     public Stellar RPC**, never against Farmledge's API;
   - printed as a QR code at the warehouse, on request, on the next visit
     and on every dispatch slip.
3. **Every month,** a statement with a balance proof for each active lot, so
   the farmer always holds a proof less than 30 days old that can be checked
   against the chain with one call.

Delivery is part of the anchoring job's definition of done:

- **Alert** if any leaf's bundle has not been delivered within 24 hours of
  its anchor.
- **Retry** failed SMS, and record a delivery receipt for each bundle.
- **Publish** a standalone open-source verifier, and the bundle
  specification, so lenders and farmers' representatives can check proofs
  without Farmledge.

## 5. Minted lots are governed on-chain (planned)

- **Minting takes the lot's balance leaf from the latest anchored day,** and
  its proof. Off-chain dispatch of a lot is **frozen** from the moment a
  mint is requested until the mint confirms or is abandoned.
- **After minting,** every dispatch of that lot goes through the contract's
  `exit`, never off-chain. Its balance leaves carry the receipt id.
- **Alert** whenever an anchored balance for a minted lot differs from its
  receipt's on-chain weight. That means grain moved outside the contract.
