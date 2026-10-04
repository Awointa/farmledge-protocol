//! Attacks from the adversarial review that no other test pinned down.
//!
//! Each test is named for what it prevents. The full review — every way a
//! farmer or a lender could lose what they are owed, and which test covers
//! it — is in `docs/adversarial-review.md`.

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, IntoVal, String,
};

use super::{next_id, setup_lending, sign_only, total_weight, Lending, DUE, GRACE, LOAN_REF};
use crate::{Error, Status};

fn loan_ref(env: &Env) -> String {
    String::from_str(env, LOAN_REF)
}

/// Locked, defaulted and sold to `buyer`. Returns the receipt id.
fn sold_to(env: &Env, l: &Lending, buyer: &Address) -> u64 {
    let id = l.f.mint(env, "DEP-SOLD");
    l.lock(env, id, 2_000);
    l.default_and_recover(env, id);
    l.complete(env, id, buyer);
    id
}

// --- farmer: signatures are bound to what was approved ---------------------

/// The farmer approves a specific quantity (SDD §5.2). Their signature for
/// a 500kg pledge must not be usable to pledge the whole receipt.
#[test]
fn an_owner_signature_for_a_small_pledge_cannot_pledge_more() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    let approved = (id, l.lender.clone(), 500u64, loan_ref(&env), DUE, GRACE);
    let farmer_auth = soroban_sdk::testutils::MockAuthInvoke {
        contract: &l.f.contract_id,
        fn_name: "lock",
        args: approved.clone().into_val(&env),
        sub_invokes: &[],
    };
    let wanted = (id, l.lender.clone(), 2_000u64, loan_ref(&env), DUE, GRACE);
    let lender_auth = soroban_sdk::testutils::MockAuthInvoke {
        contract: &l.f.contract_id,
        fn_name: "lock",
        args: wanted.into_val(&env),
        sub_invokes: &[],
    };
    env.mock_auths(&[
        soroban_sdk::testutils::MockAuth {
            address: &l.f.farmer,
            invoke: &farmer_auth,
        },
        soroban_sdk::testutils::MockAuth {
            address: &l.lender,
            invoke: &lender_auth,
        },
    ]);

    assert!(l
        .f
        .client
        .try_lock(&id, &l.lender, &2_000, &loan_ref(&env), &DUE, &GRACE)
        .is_err());
    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.weight_kg, 2_000);

    // Control: what the farmer did approve goes through.
    sign_only(
        &env,
        &l.f.contract_id,
        "lock",
        approved.into_val(&env),
        &[&l.f.farmer, &l.lender],
    );
    let locked =
        l.f.client
            .lock(&id, &l.lender, &500, &loan_ref(&env), &DUE, &GRACE);
    assert_eq!(l.f.client.get_receipt(&locked).weight_kg, 500);
}

/// The same binding for the due date: a signature for a 30-day loan cannot
/// be replayed onto a loan that falls due tomorrow.
#[test]
fn an_owner_signature_cannot_be_moved_to_an_earlier_due_date() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    let early = super::NOW + 1;

    sign_only(
        &env,
        &l.f.contract_id,
        "lock",
        (id, l.lender.clone(), 2_000u64, loan_ref(&env), DUE, GRACE).into_val(&env),
        &[&l.f.farmer, &l.lender],
    );
    assert!(l
        .f
        .client
        .try_lock(&id, &l.lender, &2_000, &loan_ref(&env), &early, &GRACE)
        .is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);

    // Control: the approved terms go through.
    l.f.client
        .lock(&id, &l.lender, &2_000, &loan_ref(&env), &DUE, &GRACE);
    assert_eq!(l.f.client.get_lock(&id).due_date, DUE);
}

/// Who receives the grain in a sale is part of what the lender and operator
/// sign. Their signatures cannot be redirected to a different buyer.
#[test]
fn a_completed_sale_cannot_be_redirected_to_another_buyer() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.default_and_recover(&env, id);
    let agreed = Address::generate(&env);
    let thief = Address::generate(&env);
    let sale_ref = String::from_str(&env, "SALE-1");

    sign_only(
        &env,
        &l.f.contract_id,
        "complete_recovery",
        (id, l.operator.clone(), agreed.clone(), sale_ref.clone()).into_val(&env),
        &[&l.operator, &l.lender],
    );
    assert!(l
        .f
        .client
        .try_complete_recovery(&id, &l.operator, &thief, &sale_ref)
        .is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
    assert_eq!(l.f.client.get_receipt(&id).owner, l.f.farmer);

    // Control: the agreed buyer goes through.
    l.f.client
        .complete_recovery(&id, &l.operator, &agreed, &sale_ref);
    assert_eq!(l.f.client.get_receipt(&id).owner, agreed);
}

// --- farmer: a partial unlock really releases ------------------------------

/// A partial unlock reduces the pledged receipt in place and releases the
/// rest as a new receipt. The lender keeps a pledge over exactly what is
/// left: it cannot touch the released receipt, and cannot unlock or sell
/// more than the remainder — either would conjure phantom grain the farmer
/// already has back.
#[test]
fn a_partial_unlock_leaves_the_lender_only_what_is_still_pledged() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    let released = l.f.client.unlock(&id, &500);

    let pledged = l.f.client.get_receipt(&id);
    assert_eq!(pledged.status, Status::Locked);
    assert_eq!(pledged.weight_kg, 1_500);
    assert_eq!(l.f.client.get_receipt(&released).status, Status::Active);

    // Nothing on the released receipt.
    assert_eq!(
        l.f.client.try_unlock(&released, &500),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(
        l.f.client.try_extend_due(&released, &(DUE + 1)),
        Err(Ok(Error::InvalidStatus))
    );
    // No more than the remainder on the pledged one.
    assert_eq!(
        l.f.client.try_unlock(&id, &2_000),
        Err(Ok(Error::InvalidSplitAmount))
    );
    env.ledger().set_timestamp(DUE + GRACE + 1);
    assert_eq!(
        l.f.client.try_start_recovery(&released),
        Err(Ok(Error::InvalidStatus))
    );
    l.f.client.start_recovery(&id);
    let buyer = soroban_sdk::Address::generate(&env);
    let sale = l.complete(&env, id, &buyer);
    assert_eq!(
        l.f.client.get_sale(&sale).weight_kg,
        1_500,
        "only the remainder is sold"
    );
    assert_eq!(l.f.client.get_receipt(&released).owner, l.f.farmer);
    assert_eq!(total_weight(&l.f, next_id(&l.f) - 1), 2_000);
}

// --- farmer: deregistration does not trap grain ----------------------------

/// Removing a warehouse stops new mints there. It must not stop owners
/// taking out grain they still hold: the custodian registrations survive and
/// still authorise exits.
#[test]
fn removing_a_warehouse_does_not_trap_the_grain_in_it() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    l.f.client.remove_warehouse(&l.f.warehouse);

    l.f.client.exit(&id, &l.f.custodian, &2_000, &1_990);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Exited);
}

/// The other side of the same rule: removing the warehouse's custodians is
/// how exits are stopped, and then no one can exit — not even the owner.
#[test]
fn removing_every_custodian_stops_exits_until_one_is_registered() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    l.f.client.remove_custodian(&l.f.warehouse, &l.f.custodian);
    assert_eq!(
        l.f.client.try_exit(&id, &l.f.custodian, &2_000, &2_000),
        Err(Ok(Error::CustodianNotRegistered))
    );

    let replacement = Address::generate(&env);
    l.f.client.add_custodian(&l.f.warehouse, &replacement);
    l.f.client.exit(&id, &replacement, &2_000, &2_000);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Exited);
}

// --- lender: collateral cannot be double-pledged or escape recovery --------

#[test]
fn a_receipt_in_recovery_cannot_be_pledged_again() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.default_and_recover(&env, id);

    let other_lender = Address::generate(&env);
    for (lender, kg) in [
        (&l.lender, 2_000u64),
        (&other_lender, 2_000),
        (&other_lender, 500),
    ] {
        assert_eq!(
            l.f.client.try_lock(
                &id,
                lender,
                &kg,
                &loan_ref(&env),
                &(DUE + GRACE + 1_000),
                &GRACE
            ),
            Err(Ok(Error::InvalidStatus))
        );
    }
    assert_eq!(l.f.client.get_lock(&id).lender, l.lender);
}

/// A lender's signature names the receipt it releases. It cannot be
/// replayed onto a different receipt pledged to the same lender.
#[test]
fn a_lender_signature_to_release_one_receipt_cannot_release_another() {
    let env = Env::default();
    let l = setup_lending(&env);
    let a = l.f.mint(&env, "DEP-A");
    let b = l.f.mint(&env, "DEP-B");
    l.lock(&env, a, 2_000);
    l.lock(&env, b, 2_000);

    sign_only(
        &env,
        &l.f.contract_id,
        "unlock",
        (a, 2_000u64).into_val(&env),
        &[&l.lender],
    );
    assert!(l.f.client.try_unlock(&b, &2_000).is_err());
    assert_eq!(l.f.client.get_receipt(&b).status, Status::Locked);

    // Control: the receipt actually named is released.
    l.f.client.unlock(&a, &2_000);
    assert_eq!(l.f.client.get_receipt(&a).status, Status::Active);
}

// --- buyer and lender after a sale -----------------------------------------

/// After a sale the grain belongs to the buyer. The former owner's signature
/// no longer moves it.
#[test]
fn the_previous_owner_cannot_exit_or_pledge_sold_grain() {
    let env = Env::default();
    let l = setup_lending(&env);
    let buyer = Address::generate(&env);
    let id = sold_to(&env, &l, &buyer);

    sign_only(
        &env,
        &l.f.contract_id,
        "exit",
        (id, l.f.custodian.clone(), 2_000u64, 2_000u64).into_val(&env),
        &[&l.f.custodian, &l.f.farmer],
    );
    assert!(l
        .f
        .client
        .try_exit(&id, &l.f.custodian, &2_000, &2_000)
        .is_err());

    let new_lender = Address::generate(&env);
    let due = env.ledger().timestamp() + 1_000;
    sign_only(
        &env,
        &l.f.contract_id,
        "lock",
        (id, new_lender.clone(), 2_000u64, loan_ref(&env), due, 0u64).into_val(&env),
        &[&l.f.farmer, &new_lender],
    );
    assert!(l
        .f
        .client
        .try_lock(&id, &new_lender, &2_000, &loan_ref(&env), &due, &0)
        .is_err());

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.owner, buyer);

    // Control: the same exit signed by the buyer goes through.
    sign_only(
        &env,
        &l.f.contract_id,
        "exit",
        (id, l.f.custodian.clone(), 2_000u64, 2_000u64).into_val(&env),
        &[&l.f.custodian, &buyer],
    );
    l.f.client.exit(&id, &l.f.custodian, &2_000, &2_000);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Exited);
}

/// A completed sale ends the lender's claim. The old lock cannot be used to
/// unlock, extend, re-start recovery or sell the grain a second time.
#[test]
fn the_lender_has_no_power_over_grain_after_its_sale() {
    let env = Env::default();
    let l = setup_lending(&env);
    let buyer = Address::generate(&env);
    let id = sold_to(&env, &l, &buyer);

    assert_eq!(
        l.f.client.try_unlock(&id, &2_000),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(
        l.f.client.try_extend_due(&id, &(DUE + 10)),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(
        l.f.client.try_start_recovery(&id),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(
        l.f.client.try_complete_recovery(
            &id,
            &l.operator,
            &Address::generate(&env),
            &String::from_str(&env, "SALE-2")
        ),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(l.f.client.get_receipt(&id).owner, buyer);
}

// --- everyone: refs are bounded ---------------------------------------------

fn text(env: &Env, len: usize) -> String {
    String::from_str(env, &"x".repeat(len))
}

/// `loan_ref`, `sale_ref` and `settlement_ref` are copied into the lock,
/// every recovery record, the sale and every receipt split off. Unbounded,
/// a lender could inflate the rent of entries the keep-alive job pays for.
/// Each is capped at `MAX_REF_LEN` bytes, and an over-long one changes
/// nothing.
#[test]
fn refs_longer_than_the_cap_are_rejected_and_change_nothing() {
    let env = Env::default();
    let l = setup_lending(&env);
    let max = crate::MAX_REF_LEN as usize;
    let id = l.f.mint(&env, "DEP-1");

    let lock = |r: &String| l.f.client.try_lock(&id, &l.lender, &2_000, r, &DUE, &GRACE);
    assert_eq!(lock(&text(&env, max + 1)), Err(Ok(Error::RefTooLong)));
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);
    assert_eq!(lock(&text(&env, max)), Ok(Ok(id)));

    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);
    assert_eq!(
        l.f.client
            .try_complete_recovery(&id, &l.operator, &buyer, &text(&env, max + 1)),
        Err(Ok(Error::RefTooLong))
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
    let sale =
        l.f.client
            .complete_recovery(&id, &l.operator, &buyer, &text(&env, max));

    assert_eq!(
        l.f.client
            .try_record_settlement(&sale, &l.operator, &text(&env, max + 1)),
        Err(Ok(Error::RefTooLong))
    );
    assert_eq!(l.f.client.get_sale(&sale).settlement_ref, None);
    l.f.client
        .record_settlement(&sale, &l.operator, &text(&env, max));
    assert_eq!(
        l.f.client.get_sale(&sale).settlement_ref,
        Some(text(&env, max))
    );
}

/// `deposit_id` is capped like the lending refs. A registered custodian
/// supplies it, but in Phase 1 custodian keys are held by the backend, so
/// registration alone does not bound it. An over-long one mints nothing.
#[test]
fn a_deposit_id_longer_than_the_cap_is_rejected_and_mints_nothing() {
    let env = Env::default();
    let l = setup_lending(&env);
    let max = crate::MAX_REF_LEN as usize;
    let mint = |deposit: &String| {
        l.f.client.try_mint(
            &l.f.custodian,
            &l.f.verifier,
            deposit,
            &l.f.farmer,
            &l.f.warehouse,
            &soroban_sdk::Symbol::new(&env, super::COMMODITY),
            &soroban_sdk::Symbol::new(&env, super::GRADE),
            &2_000,
            &crate::WeightSource::Scale,
            &20,
            &super::MOISTURE,
        )
    };

    assert_eq!(mint(&text(&env, max + 1)), Err(Ok(Error::RefTooLong)));
    assert_eq!(l.f.client.owner_page_count(&l.f.farmer), 0);
    assert_eq!(next_id(&l.f), 1, "no receipt id was allocated");

    assert_eq!(mint(&text(&env, max)), Ok(Ok(1)));
}
