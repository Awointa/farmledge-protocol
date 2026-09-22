#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, String, Symbol};

use crate::storage::DataKey;
use crate::{ReceiptContract, ReceiptContractClient, Status, WeightSource};

const WAREHOUSE: &str = "WH_KANO_01";
const COMMODITY: &str = "MAIZE_WHITE";
const GRADE: &str = "GRADE_1";

struct Fixture<'a> {
    client: ReceiptContractClient<'a>,
    custodian: Address,
    verifier: Address,
    owner: Address,
}

/// Registers a custodian/verifier directly in contract storage, bypassing
/// the still-unimplemented `add_custodian`/`add_verifier` admin functions
/// (see admin.rs) so mint()'s real logic can be exercised in isolation.
fn setup(env: &Env) -> Fixture<'_> {
    let contract_id = env.register(ReceiptContract, ());
    let client = ReceiptContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let custodian = Address::generate(env);
    let verifier = Address::generate(env);
    let owner = Address::generate(env);

    env.mock_all_auths();
    client.initialize(&admin);

    let warehouse_id = Symbol::new(env, WAREHOUSE);
    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::Custodian(warehouse_id, custodian.clone()), &true);
        env.storage()
            .persistent()
            .set(&DataKey::Verifier(verifier.clone()), &true);
    });

    Fixture {
        client,
        custodian,
        verifier,
        owner,
    }
}

#[allow(clippy::too_many_arguments)]
fn mint_default(env: &Env, f: &Fixture, deposit_id: &str) -> u64 {
    f.client.mint(
        &f.custodian,
        &f.verifier,
        &String::from_str(env, deposit_id),
        &Symbol::new(env, COMMODITY),
        &Symbol::new(env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &1250u32,
        &Symbol::new(env, WAREHOUSE),
        &f.owner,
    )
}

// --- Real, passing tests (proves the harness) ------------------------------

#[test]
fn mint_creates_an_active_receipt() {
    let env = Env::default();
    let f = setup(&env);

    let receipt_id = mint_default(&env, &f, "DEP-0001");
    let receipt = f.client.get_receipt(&receipt_id);

    assert_eq!(receipt.receipt_id, receipt_id);
    assert_eq!(receipt.status, Status::Active);
    assert_eq!(receipt.weight_kg, 2_000);
    assert_eq!(receipt.weight_source, WeightSource::Scale);
    assert_eq!(receipt.owner, f.owner);
    assert!(receipt.parent_id.is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn deposit_id_cannot_be_minted_twice() {
    let env = Env::default();
    let f = setup(&env);

    mint_default(&env, &f, "DEP-0001");
    // Same deposit_id again: the contract returns Err(DuplicateDepositId),
    // and the generated client's infallible `mint()` panics on any Err.
    mint_default(&env, &f, "DEP-0001");
}

/// Proves auth is actually enforced, not just checked in name: the
/// custodian and verifier are registered directly in storage (bypassing
/// `setup()`, which mocks all auths for its Env), and mint() is called
/// against a fresh Env with no authorizations mocked at all. custodian's
/// own `require_auth()` — the first auth check in mint() — must trap.
#[test]
#[should_panic]
fn mint_requires_verifier_auth() {
    let env = Env::default();
    let contract_id = env.register(ReceiptContract, ());
    let client = ReceiptContractClient::new(&env, &contract_id);

    let custodian = Address::generate(&env);
    let verifier = Address::generate(&env);
    let owner = Address::generate(&env);
    let warehouse_id = Symbol::new(&env, WAREHOUSE);

    env.as_contract(&contract_id, || {
        env.storage().persistent().set(
            &DataKey::Custodian(warehouse_id.clone(), custodian.clone()),
            &true,
        );
        env.storage()
            .persistent()
            .set(&DataKey::Verifier(verifier.clone()), &true);
    });

    client.mint(
        &custodian,
        &verifier,
        &String::from_str(&env, "DEP-AUTH"),
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &1250u32,
        &warehouse_id,
        &owner,
    );
}

// --- Placeholder tests: one per remaining function, marked TODO ------------
// Each is #[ignore] so `cargo test` passes; run with `-- --ignored` once the
// corresponding function body is implemented.

#[test]
#[ignore = "TODO: implement lock() — splits if kg < weight_kg, requires Scale+Active"]
fn lock_locks_a_scale_weighed_active_receipt() {
    todo!()
}

#[test]
#[ignore = "TODO: implement unlock() — partial release splits, full release clears lock"]
fn unlock_releases_locked_collateral() {
    todo!()
}

#[test]
#[ignore = "TODO: implement extend_due() — can only move the due date later"]
fn extend_due_moves_date_later_only() {
    todo!()
}

#[test]
#[ignore = "TODO: implement start_recovery() — only after due_date + grace_period"]
fn start_recovery_enters_recovery_status() {
    todo!()
}

#[test]
#[ignore = "TODO: implement complete_recovery() — transfers receipt to buyer post-escrow"]
fn complete_recovery_transfers_to_buyer() {
    todo!()
}

#[test]
#[ignore = "TODO: implement exit() — splits if partial, records exit_weight_kg"]
fn exit_releases_goods_and_records_weight() {
    todo!()
}

#[test]
#[ignore = "TODO: implement revoke() — admin multisig only, any status -> Revoked"]
fn revoke_marks_receipt_revoked() {
    todo!()
}

#[test]
#[ignore = "TODO: implement extend_ttl() — bumps persistent TTL, no auth required"]
fn extend_ttl_bumps_storage_ttl() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::add_custodian() — per-warehouse registration"]
fn add_custodian_registers_for_warehouse() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::remove_custodian() — per-warehouse deregistration"]
fn remove_custodian_deregisters_for_warehouse() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::add_verifier()"]
fn add_verifier_registers_globally() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::remove_verifier()"]
fn remove_verifier_deregisters_globally() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::add_recovery_operator()"]
fn add_recovery_operator_registers_globally() {
    todo!()
}

#[test]
#[ignore = "TODO: implement admin::remove_recovery_operator()"]
fn remove_recovery_operator_deregisters_globally() {
    todo!()
}

// --- Placeholder tests: one per invariant (design doc §4.3), marked TODO ---
// "deposit_id cannot be minted twice" is already covered above as a real test.

#[test]
#[ignore = "TODO: invariant — splits always sum to the parent weight"]
fn splits_always_sum_to_parent_weight() {
    todo!()
}

#[test]
#[ignore = "TODO: invariant — a LOCKED receipt cannot be exited"]
fn locked_receipts_cannot_be_exited() {
    todo!()
}

#[test]
#[ignore = "TODO: invariant — only the lender can unlock or start recovery"]
fn only_lender_can_unlock_or_start_recovery() {
    todo!()
}

#[test]
#[ignore = "TODO: invariant — recovery cannot start before due_date + grace_period"]
fn recovery_cannot_start_before_due_plus_grace() {
    todo!()
}
