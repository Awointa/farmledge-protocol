# Adversarial review

Every way we could find for a party to lose what it is rightfully owed, and
the test that stops it. Test names are `file::test` under
`contracts/receipt/src/test/`. **NEW** marks a test added in this review,
for a case that was previously uncovered.

The last section lists losses the contract **cannot** prevent. Those rest on
trust in a named party or on an off-chain process, and each needs an owner
outside this repo.

## a. A farmer loses grain they still rightfully own

| # | How | Stopped by | Test |
|---|---|---|---|
| F1 | Grain released without the owner's signature | `exit` requires the owner's auth | `exit::exit_fails_with_only_the_custodian_signature` |
| F2 | A custodian of another warehouse releases it | custodian must be registered for the receipt's warehouse | `exit::exit_rejects_a_custodian_from_another_warehouse` |
| F3 | Pledged without the farmer's consent | `lock` requires the owner's auth | `lock::lock_fails_with_only_the_lenders_signature` |
| F4 | The farmer approves 500 kg, and the approval is used to pledge 2,000 kg | auth is bound to the call's arguments | **NEW** `adversarial::an_owner_signature_for_a_small_pledge_cannot_pledge_more` |
| F5 | The approval is replayed with an earlier due date (instant default) | same | **NEW** `adversarial::an_owner_signature_cannot_be_moved_to_an_earlier_due_date` |
| F6 | A partial pledge takes more than requested, or the free remainder is lost or tied up | split gives the remainder back as an independent `Active` receipt | `lock::partial_lock_splits_and_leaves_the_remainder_active_under_the_owner`, `unlock::acting_on_the_free_remainder_never_touches_the_locked_portion`, `unlock::acting_on_the_locked_portion_never_touches_the_free_remainder` |
| F7 | Recovery starts before due date + grace | strict `now > due + grace` | `recovery::start_recovery_before_due_plus_grace_fails_and_one_second_after_succeeds` |
| F8 | Someone other than the recorded lender starts recovery | recorded lender's auth | `recovery::start_recovery_by_the_owner_or_another_lender_fails` |
| F9 | The lender shortens the loan | `extend_due` only moves later | `unlock::extend_due_to_an_earlier_or_equal_date_fails` |
| F10 | A loan already past due, or one whose `due + grace` overflows | `lock` validation | `lock::lock_rejects_a_due_date_that_is_not_in_the_future_or_that_overflows` |
| F11 | The lender sells without the operator (no escrow) | operator co-signs | `recovery::complete_recovery_with_only_the_lender_fails` |
| F12 | An unregistered address poses as the operator | operator registry | `recovery::an_unregistered_recovery_operator_fails` |
| F13 | Sold without a recovery having started | `Recovery` status required | `recovery::complete_recovery_from_locked_fails`, `recovery::complete_recovery_from_active_fails` |
| F14 | The signed sale is redirected to a different buyer | auth bound to `buyer` | **NEW** `adversarial::a_completed_sale_cannot_be_redirected_to_another_buyer` |
| F15 | Unlocking during recovery hands the grain to the lender | released grain goes to `owner` | `recovery::unlock_during_recovery_returns_the_receipt_to_the_owner_not_the_lender` |
| F16 | The lender acts on a retired `Split` parent that still carries its lock, creating phantom grain or selling grain already released | status gate before any lender action | **NEW** `adversarial::a_lender_cannot_act_on_a_retired_split_parent`, `prop_lifecycle` |
| F17 | A split loses kilograms or bags | one split helper, conservation | `prop_split::*`, `prop_lifecycle` (invariant 1) |
| F18 | The admin voids a pledged receipt | revoke is `Active`-only | `lock::revoke_from_locked_fails`, `recovery::exit_and_revoke_on_a_receipt_in_recovery_fail` |
| F19 | A non-admin voids a receipt | stored admin's auth | `revoke::revoke_is_admin_only` |
| F20 | Deregistering a warehouse traps the grain in it | custodian registrations survive and still authorise exits | **NEW** `adversarial::removing_a_warehouse_does_not_trap_the_grain_in_it` |
| F21 | Split children don't show up in the owner's list | both children indexed | `exit::split_children_are_indexed_for_owner_and_warehouse` |
| F22 | A receipt is archived and treated as lost | persistent entries restore intact | **NEW** `ttl::an_archived_receipt_is_restored_intact_and_needs_extending_again` |
| F23 | A full index page ages out while the index is in use | `extend_index_ttl` | **NEW** `ttl::a_full_index_page_ages_even_while_the_index_is_in_use` |
| F24 | Someone claims the contract between deploy and `initialize` and registers their own custodians | **fixed:** a constructor installs the admin in the deploy transaction | **NEW** `registry::the_constructor_installs_the_admin_in_the_deploy_itself` |
| F25 | The Wasm is replaced to rewrite ownership | contract is immutable | `make check-immutable` (CI) |
| F26 | A rejected call leaves partial state behind | all-or-nothing calls | `prop_lifecycle` (no side effects on rejection) |

## b. A lender loses collateral it is rightfully owed

| # | How | Stopped by | Test |
|---|---|---|---|
| L1 | The owner exits locked grain | `Active`-only exit | `lock::exit_on_a_locked_receipt_fails` |
| L2 | The owner exits during recovery | same | `recovery::exit_and_revoke_on_a_receipt_in_recovery_fail` |
| L3 | The admin voids pledged collateral | revoke is `Active`-only | `lock::revoke_from_locked_fails` |
| L4 | The owner releases their own collateral | recorded lender's auth | `unlock::unlock_by_the_owner_fails`, `recovery::unlock_during_recovery_by_the_owner_fails` |
| L5 | A different lender releases it | same | `unlock::unlock_by_a_different_lender_fails` |
| L6 | Double pledge while `Locked` | `Active`-only lock | `lock::lock_on_an_already_locked_receipt_fails` |
| L7 | Double pledge while in `Recovery` | same | **NEW** `adversarial::a_receipt_in_recovery_cannot_be_pledged_again` |
| L8 | The owner re-uses weight through a retired `Split` parent | `Split` is terminal | `lock::lock_rejects_exited_revoked_and_split_receipts`, `exit::exit_rejects_anything_not_active` |
| L9 | A lender's release signature is replayed onto a different receipt | auth bound to `receipt_id` | **NEW** `adversarial::a_lender_signature_to_release_one_receipt_cannot_release_another` |
| L10 | A partial release frees more than intended, or the remainder loses its pledge | split keeps lock and recovery on the remainder | `unlock::partial_unlock_releases_a_portion_and_keeps_the_rest_locked`, `recovery::partial_unlock_during_recovery_keeps_the_rest_in_recovery`, `unlock::unlock_rejects_impossible_quantities` |
| L11 | Still-pledged split children drop out of the lender's list | children of pledged receipts are indexed under the lender | `lock::locked_receipts_are_indexed_under_the_lender` |
| L12 | The farmer extends the due date | recorded lender's auth | `unlock::extend_due_by_the_owner_or_another_lender_fails` |
| L13 | The operator sells alone | lender co-signs | `recovery::complete_recovery_with_only_the_operator_fails` |
| L14 | The operator records a settlement alone | lender co-signs (see question Q1) | `settlement::record_settlement_with_only_the_operator_fails`, `settlement::record_settlement_with_the_operator_and_a_different_lender_fails` |
| L15 | A settlement is overwritten | write-once | `settlement::a_settlement_is_write_once`, `prop_lifecycle` (invariant 3) |
| L16 | After the buyer splits, a settlement attaches to the wrong sale or a child can't resolve it | settlement lives on the canonical `Sale` | `settlement::a_settlement_recorded_after_the_buyer_splits_reaches_every_child`, `prop_lifecycle` (invariant 4) |
| L17 | Two sales of the same lineage get confused | one `sale_id` per sale | `settlement::a_receipt_sold_twice_has_two_independently_settled_sales` |
| L18 | The previous owner still moves grain that was sold (a loss for the buyer, and so for the lender's recovery) | ownership passes to the buyer | **NEW** `adversarial::the_previous_owner_cannot_exit_or_pledge_sold_grain` |
| L19 | The old lock is reused after the sale | lock cleared on completion | **NEW** `adversarial::the_lender_has_no_power_over_grain_after_its_sale` |
| L20 | Collateral, the sale record or the contract instance is archived | keep-alive calls, instance bumped by every call | `settlement::extend_ttl_keeps_the_sale_record_alive`, `ttl::extend_ttl_batch_*`, **NEW** `ttl::keep_alive_calls_refresh_the_contract_instance`, **NEW** `ttl::state_changes_refresh_the_contract_instance` |
| L21 | The lender can't find its sales to reconcile settlements | `get_sales_by_lender` | **NEW** `reads::sales_are_listed_per_lender_settled_or_not`, **NEW** `reads::lender_sale_pages_roll_over_at_64` |
| L22 | The indexer loses track of collateral across a split of a pledged receipt | `split` now carries `lender` | `unlock::unlocked_event_marks_a_partial_release` (updated) |
| L23 | Deploy-time hijack, or a Wasm upgrade | as F24, F25 | as F24, F25 |

## Losses the contract cannot prevent

These need an owner outside this repo.

| # | Risk | Who must control it |
|---|---|---|
| T1 | **The lender can block, and nothing on-chain checks, the farmer's surplus.** `record_settlement` needs the lender's signature and records only a `settlement_ref`. SDD §4.2 says the operator signs alone and records `amount_to_lender` and `amount_to_farmer`, "so surplus return is verifiable rather than merely claimed". As built, a lender who refuses to co-sign leaves the sale unsettled forever, and no amounts are on-chain. | **Question Q1.** The reconciliation job flags unsettled sales (SDD §5.5), which makes the gap visible but does not close it. |
| T2 | The admin revokes a legitimate `Active` receipt. Revoke is intended for fraud, and the contract cannot tell fraud from abuse. | Admin 2-of-3 multisig. `revoked` events carry the reason, and the indexer should alert on every revoke. |
| T3 | The admin removes every custodian of a warehouse, and no one can exit until one is registered (pinned by `adversarial::removing_every_custodian_stops_exits_until_one_is_registered`). The same goes for recovery operators: with none, no recovery can complete. | Admin multisig. Treat it as a liveness dependency, not a theft vector, because no grain changes hands. |
| T4 | Lender and operator collude on a low-price sale or a related buyer. | Escrow bank, recovery-buyer identity checks, sale window (SDD §5.5). The contract records `sale_ref` and `buyer`. |
| T5 | The lender does not unlock after repayment. | Loan agreement and legal remedy. The contract deliberately gives no one else the power to release collateral. |
| T6 | Farmledge holds farmer, custodian and buyer keys, so co-signatures are procedural rather than cryptographic. | SDD §6 upgrade path: separate key material, then self-custody before real lending. |
| T7 | A custodian and verifier collude to mint a receipt for grain that does not exist. | Physical controls (SDD §1.1). On discovery, the lender unlocks and the admin revokes. |
| T8 | An archived entry costs a restore before it can be used. Persistent data is never deleted, so this is a cost, not a loss. | Keep-alive job (docs/archival.md). |

## Test quality: which tests would pass with their rule removed

Two methods:

1. **Mutation testing** (`cargo mutants`, 215 mutants of the contract
   source, tests excluded). Each surviving mutant is a rule that no test
   enforces.
2. **Deleting each `require_auth()`** one at a time and recording which
   tests fail (cargo-mutants never deletes statements, so it can't check
   this).

Findings on the code as it was at the start of this review, and what was
done about each:

| Rule | What would still pass if it were removed | Fix |
|---|---|---|
| Moisture bound is `>`, not `>=` (100.00% is a valid reading) | `mint::mint_rejects_invalid_measurements` only tested 10,001 | boundary assertion added |
| Every registry change emits its event (8 functions) | all registry tests; no test looked at registry events | **NEW** `registry::every_registry_change_emits_its_event_with_a_timestamp` |
| Instance TTL is bumped, and the instance threshold is 30 days | every TTL test | **NEW** `ttl::keep_alive_calls_refresh_the_contract_instance`, `ttl::state_changes_refresh_the_contract_instance` |
| `warehouse_page_count` counts pages | `mint::mint_indexes_the_receipt_by_owner_and_warehouse` (one page only) | **NEW** `reads::owner_and_warehouse_reads_are_paged_and_bounded` |
| `prop_lending::no_sequence_of_calls_changes_total_weight`: claimed to cover recovery | It picked targets uniformly at random, so it completed **one** recovery in 64 cases and never settled anything. Removing settlement write-once, or breaking sale lineage, still passed. | Replaced by `prop_lifecycle`, which picks a receipt in the status the operation accepts 80% of the time. Verified by injecting three bugs (settlement written twice, sale pointing at the wrong receipt, unlock leaving the lock on an `Active` receipt): all three now fail the property. |
| `registry::initialize_runs_once`, `registry::registry_calls_before_initialize_are_rejected` | n/a: `initialize` no longer exists | deleted, replaced by the constructor test |
| `exit` / `lock` / `unlock`: `kg == 0 \|\| kg > weight` mutated to `&&` | every test, **correctly**: `split_receipt` re-checks `0 < kg < weight` and returns the same error, so these are equivalent mutants | none needed. Documented here so nobody "fixes" the defence in depth away. |

A second mutation run on the hardened code (241 mutants, new code included)
found these survivors:

| Rule | What would still pass if it were removed | Fix |
|---|---|---|
| `extend_index_ttl` accepts exactly `MAX_TTL_BATCH` pages (`>`, not `>=`) | `ttl::extend_index_ttl_covers_every_index_kind_and_is_bounded` only tested 26 | boundary assertion added; the mutant now fails it |
| `exit`: `kg == 0 \|\| kg > weight` mutated to `&&` | equivalent, as above | none |
| `data_version()` replaced with `1` | equivalent while `DATA_VERSION` is 1. `reads` asserts it equals the constant, which is what a future bump needs. | none |

Every `require_auth()` deletion is caught by at least one targeted test. See
the table below, generated by deleting each call.

| deleted call | tests that fail |
|---|---|
| `lending.rs:59` `lock.lender.require_auth();` | `adversarial::a_lender_signature_to_release_one_receipt_cannot_release_another`, `recovery::complete_recovery_with_only_the_operator_fails`, `recovery::start_recovery_by_the_owner_or_another_lender_fails`, `recovery::unlock_during_recovery_by_the_owner_fails`, `unlock::extend_due_by_the_owner_or_another_lender_fails`, `unlock::unlock_by_the_owner_fails`, `unlock::unlock_by_a_different_lender_fails` |
| `lending.rs:86` `lender.require_auth();` | `lock::lock_fails_with_only_the_owners_signature` |
| `lending.rs:87` `receipt.owner.require_auth();` | `adversarial::an_owner_signature_for_a_small_pledge_cannot_pledge_more`, `adversarial::the_previous_owner_cannot_exit_or_pledge_sold_grain`, `lock::lock_fails_with_only_the_lenders_signature` |
| `lending.rs:274` `operator.require_auth();` | `recovery::complete_recovery_with_only_the_lender_fails` |
| `lending.rs:350` `operator.require_auth();` | `settlement::record_settlement_with_only_the_lender_fails` |
| `lending.rs:351` `sale.lender.require_auth();` | `settlement::a_receipt_sold_twice_has_two_independently_settled_sales`, `settlement::record_settlement_with_only_the_operator_fails`, `settlement::record_settlement_with_the_operator_and_a_different_lender_fails` |
| `admin.rs:30` `admin.require_auth();` | `registry::a_non_admin_cannot_touch_any_registry`, `registry::the_constructor_installs_the_admin_in_the_deploy_itself`, `revoke::revoke_is_admin_only` |
| `lib.rs:97` `custodian.require_auth();` | `mint::mint_fails_with_only_the_verifier_signature` |
| `lib.rs:98` `verifier.require_auth();` | `mint::mint_fails_with_only_the_custodian_signature` |
| `lib.rs:199` `custodian.require_auth();` | `exit::exit_fails_with_only_the_owner_signature` |
| `lib.rs:200` `receipt.owner.require_auth();` | `exit::exit_fails_with_only_the_custodian_signature`, `adversarial::the_previous_owner_cannot_exit_or_pledge_sold_grain`, `recovery::the_buyer_can_exit_after_a_completed_recovery` |

The `.is_err()`-only negative auth tests were reviewed. Each is paired with
a positive control in the same test or file (for example
`unlock_by_the_owner_fails` with `unlock_by_the_recorded_lender_succeeds`),
so a mis-built mock cannot make them pass vacuously. The new binding tests
in `adversarial.rs` carry their own positive control.
