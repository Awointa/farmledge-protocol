# Upgrade strategy

**Decision: the receipt contract is immutable.** It has no entry point that
replaces its Wasm. A change ships as a new deployment, and receipts move to
it through the drain-and-re-mint migration described below. Every stored
`Receipt` and `Sale` carries a `data_version` (currently `2`, readable with
`data_version()`), so any future tool or contract can tell which layout a
record was written in. Version 1 stored named-field maps; version 2 stores
the positional layout in `src/layout.rs`.

## Why not admin-upgradeable

On Soroban, a contract can be upgraded only if it exposes a function that
calls `env.deployer().update_current_contract_wasm(hash)`. Whoever can call
that function can replace every rule in the contract with any other rule, in
one transaction. For a contract holding warehouse receipts, that power is
strictly greater than every protection the design relies on:

| The design promises (SDD) | An upgrade key can |
|---|---|
| "Farmledge cannot destroy collateral on its own": revoke is Active-only, and a pledged receipt must be unlocked by its lender first (§4.2) | ship code that revokes a Locked receipt |
| Only the lender can release a lock (§1.2) | ship code that unlocks without the lender |
| A tamper-evident history "Farmledge cannot silently rewrite" (§1.1) | ship code that rewrites `owner`, `weight_kg` or `recovery_history` |
| The design test: "If Farmledge disappeared tomorrow, could a lender still verify and enforce its collateral?" | a lender's security depends on whoever holds the upgrade key, forever |

The admin is a 2-of-3 Farmledge multisig. That makes a rogue upgrade harder,
but the people who hold the key are still Farmledge, and the lender is the
party the design is meant to protect from Farmledge. A timelock (announce the
upgrade, wait, apply) would not fix this. The only thing a lender can do with
a locked receipt during the wait is unlock it, which means giving up the
collateral the timelock was supposed to protect.

The deciding question is whether a lender will accept collateral that
someone else can redefine. A lender's counsel will probe exactly this (SDD
§6 already flags key custody as "the first thing a lender will probe").
"No one can change these rules, including Farmledge" is an answer that holds
up. "Three Farmledge staff can change them" is not.

## What immutability costs

- **Bugs cannot be patched in place.** A bug in a deployed contract stays
  there for every receipt that still lives in it. Mitigations: the test
  suite, including the property tests in `src/test/prop_*.rs`, mutation
  testing, an external audit before Mainnet lending, and the fact that only
  receipts in a live loan are hard to move (see below).
- **Migration needs the parties' signatures**, not just the admin's. That
  cost is deliberate.
- **The backend has to index more than one contract** during a migration.
  The indexer already keys everything by contract id, so this means a list
  of ids rather than one.

On Testnet none of this matters much. Testnet is reset periodically anyway,
and every reset is a redeploy (see the README).

## Migration path: drain and re-mint

When a v2 contract is needed:

1. **Deploy v2** with `make deploy-testnet` (or the Mainnet equivalent) and
   register the warehouses, custodians, verifiers and recovery operators on
   it.
2. **Stop new receipts in v1.** The admin removes every **verifier** from
   v1. Every mint needs a registered verifier, so no new receipt can be
   created there. Do **not** remove v1 custodians: exits still need them.
   New intake now mints in v2.
3. **Let free receipts move.** For each `Active` receipt in v1, the owner
   and a custodian `exit` it in v1, recording the full book weight with
   `exit_weight_kg = weight_kg` because no grain physically leaves. The
   custodian and verifier then `mint` it in v2 with the same `deposit_id`,
   commodity, grade and weight. `deposit_id` uniqueness is per contract, so
   the re-mint is accepted, and the v1 `Exited` record is the audit link.
   The backend can batch this per warehouse, with farmer confirmation as for
   any exit.
4. **Let pledged receipts finish in v1.** `Locked` and `Recovery` receipts
   stay in v1 until their lender unlocks them (then they are `Active` and
   move as in step 3) or they are sold (then the buyer's receipt moves as in
   step 3). No one, Farmledge included, can move a pledged receipt without
   its lender. That is the property the immutability preserves. Typical
   loans run for months, so v1 drains within one loan cycle.
5. **Keep v1 alive.** Its keep-alive job keeps running until v1 holds no
   `Active`, `Locked` or `Recovery` receipt. Its terminal records stay
   readable and restorable as the audit trail.

`data_version` is not needed for this path: v2 mints fresh records. It is
there for the alternatives: a v2 that reads v1 records directly through a
cross-contract call, or an off-chain tool that exports v1 state for an
auditor. Either has to know which layout it is decoding.

### When a fix cannot wait

If v1 has a bug that endangers receipts, the same steps apply, faster. The
admin removes v1's verifiers immediately (no new exposure), and the backend
asks lenders to unlock and re-pledge in v2. The contract cannot force a
lender's hand. That is the guarantee, and also the cost.

## What the code does to keep this true

- No function in `src/` calls `update_current_contract_wasm`, and CI fails
  if one appears (see `.github/workflows/ci.yml`, "Contract is immutable").
- The admin is installed by the constructor in the deploy transaction, so
  there is no unowned window in which someone else could claim the contract.
- `DATA_VERSION` in `src/types.rs` must be bumped whenever a stored type's
  fields change, and that includes the field order and codes in
  `src/layout.rs`, which are the storage layout.

## If you choose upgradeability anyway

If the business decides that the ability to hot-fix beats the guarantee
above (for example, for a Testnet-only phase), the minimal safe shape is:

- `upgrade(new_wasm_hash)`, admin only, gated on the admin multisig.
- A `migrate()` that reads each record's `data_version` and rewrites old
  layouts lazily on first touch. That is why the field exists now.
- An `Upgraded { old_hash, new_hash, at }` event, so lenders' monitors see it.
- Written disclosure to every lender that the contract can be changed.

This repo does not take that path.
