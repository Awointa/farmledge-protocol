use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, IntoVal, String,
};

use super::{setup_lending, sign_only, Lending, DUE, GRACE, LOAN_REF};
use crate::{Error, MaybeLock, MaybeRecovery, Recovery, RecoveryResolution, Status};

const DEADLINE: u64 = DUE + GRACE;

fn locked_receipt(env: &Env, l: &Lending) -> u64 {
    let id = l.f.mint(env, "DEP-1");
    l.lock(env, id, 2_000);
    id
}

fn complete_args(
    env: &Env,
    l: &Lending,
    id: u64,
    buyer: &Address,
) -> soroban_sdk::Vec<soroban_sdk::Val> {
    (
        id,
        l.operator.clone(),
        buyer.clone(),
        String::from_str(env, "SALE-1"),
    )
        .into_val(env)
}

fn try_complete(
    env: &Env,
    l: &Lending,
    id: u64,
    operator: &Address,
    buyer: &Address,
) -> Result<u64, Result<Error, soroban_sdk::InvokeError>> {
    l.f.client
        .try_complete_recovery(&id, operator, buyer, &String::from_str(env, "SALE-1"))
        .map(|r| r.unwrap())
}

// --- start_recovery: authorisation and timing ------------------------------

#[test]
fn start_recovery_by_the_owner_or_another_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    env.ledger().set_timestamp(DEADLINE + 1);
    let other_lender = Address::generate(&env);

    for signer in [&l.f.farmer, &other_lender] {
        sign_only(
            &env,
            &l.f.contract_id,
            "start_recovery",
            (id,).into_val(&env),
            &[signer],
        );
        assert!(l.f.client.try_start_recovery(&id).is_err());
        assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
    }

    sign_only(
        &env,
        &l.f.contract_id,
        "start_recovery",
        (id,).into_val(&env),
        &[&l.lender],
    );
    l.f.client.start_recovery(&id);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

#[test]
fn start_recovery_before_due_plus_grace_fails_and_one_second_after_succeeds() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);

    // Before due, at due, inside grace, and at the exact deadline: all early.
    for now in [DUE - 1, DUE, DUE + 1, DEADLINE - 1, DEADLINE] {
        env.ledger().set_timestamp(now);
        assert_eq!(
            l.f.client.try_start_recovery(&id),
            Err(Ok(Error::RecoveryNotYetDue)),
            "at t={now}"
        );
    }
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);

    env.ledger().set_timestamp(DEADLINE + 1);
    l.f.client.start_recovery(&id);

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Recovery);
    assert_eq!(
        r.recovery,
        MaybeRecovery::Some(Recovery {
            lender: l.lender.clone(),
            loan_ref: String::from_str(&env, LOAN_REF),
            owner: l.f.farmer.clone(),
            started_at: DEADLINE + 1,
            resolution: RecoveryResolution::Open,
            resolved_at: None,
            sale_id: None,
            buyer: None,
            sale_ref: None,
        })
    );
    // The pledge is still readable while recovery runs.
    assert_eq!(l.f.client.get_lock(&id).lender, l.lender);
}

#[test]
fn extend_due_moves_the_recovery_deadline() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.f.client.extend_due(&id, &(DUE + 1_000));

    env.ledger().set_timestamp(DEADLINE + 1);
    assert_eq!(
        l.f.client.try_start_recovery(&id),
        Err(Ok(Error::RecoveryNotYetDue))
    );
    env.ledger().set_timestamp(DEADLINE + 1_001);
    l.f.client.start_recovery(&id);
}

#[test]
fn start_recovery_rejects_anything_not_locked() {
    let env = Env::default();
    let l = setup_lending(&env);
    env.ledger().set_timestamp(DEADLINE + 1);

    let free = l.f.mint(&env, "DEP-FREE");
    assert_eq!(
        l.f.client.try_start_recovery(&free),
        Err(Ok(Error::InvalidStatus))
    );

    env.ledger().set_timestamp(super::NOW);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    assert_eq!(
        l.f.client.try_start_recovery(&id),
        Err(Ok(Error::InvalidStatus)),
        "already in recovery"
    );
}

// --- complete_recovery: authorisation --------------------------------------

#[test]
fn complete_recovery_with_only_the_operator_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);

    let args = complete_args(&env, &l, id, &buyer);
    sign_only(
        &env,
        &l.f.contract_id,
        "complete_recovery",
        args,
        &[&l.operator],
    );
    assert!(try_complete(&env, &l, id, &l.operator, &buyer).is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

#[test]
fn complete_recovery_with_only_the_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);

    let args = complete_args(&env, &l, id, &buyer);
    sign_only(
        &env,
        &l.f.contract_id,
        "complete_recovery",
        args,
        &[&l.lender],
    );
    assert!(try_complete(&env, &l, id, &l.operator, &buyer).is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

#[test]
fn complete_recovery_with_operator_and_lender_succeeds() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);

    let args = complete_args(&env, &l, id, &buyer);
    sign_only(
        &env,
        &l.f.contract_id,
        "complete_recovery",
        args,
        &[&l.operator, &l.lender],
    );
    assert!(try_complete(&env, &l, id, &l.operator, &buyer).is_ok());
}

#[test]
fn an_unregistered_recovery_operator_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);

    // Both parties sign, so this isolates the registry check.
    let impostor = Address::generate(&env);
    assert_eq!(
        try_complete(&env, &l, id, &impostor, &buyer),
        Err(Ok(Error::RecoveryOperatorNotRegistered))
    );

    // A deregistered operator is as good as an unregistered one.
    l.f.client.remove_recovery_operator(&l.operator);
    assert_eq!(
        try_complete(&env, &l, id, &l.operator, &buyer),
        Err(Ok(Error::RecoveryOperatorNotRegistered))
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

// --- complete_recovery: state ----------------------------------------------

#[test]
fn complete_recovery_from_locked_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    env.ledger().set_timestamp(DEADLINE + 1); // overdue, but never started
    let buyer = Address::generate(&env);

    assert_eq!(
        try_complete(&env, &l, id, &l.operator, &buyer),
        Err(Ok(Error::InvalidStatus))
    );
    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Locked);
    assert_eq!(r.owner, l.f.farmer);
}

#[test]
fn complete_recovery_from_active_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    let buyer = Address::generate(&env);

    assert_eq!(
        try_complete(&env, &l, id, &l.operator, &buyer),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(l.f.client.get_receipt(&id).owner, l.f.farmer);
}

#[test]
fn complete_recovery_hands_the_receipt_to_the_buyer_and_records_the_sale() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    env.ledger().set_timestamp(DEADLINE + 500);
    let buyer = Address::generate(&env);
    let sale_id = l.complete(&env, id, &buyer);
    assert_eq!(sale_id, 1, "sale ids start at 1");

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.owner, buyer);
    assert_eq!(r.lock, MaybeLock::None);
    assert_eq!(r.recovery, MaybeRecovery::None);
    assert_eq!(r.recovery_history.len(), 1);
    assert_eq!(
        r.recovery_history.get(0).unwrap(),
        Recovery {
            lender: l.lender.clone(),
            loan_ref: String::from_str(&env, LOAN_REF),
            owner: l.f.farmer.clone(),
            started_at: DEADLINE + 1,
            resolution: RecoveryResolution::Completed,
            resolved_at: Some(DEADLINE + 500),
            sale_id: Some(sale_id),
            buyer: Some(buyer.clone()),
            sale_ref: Some(String::from_str(&env, "SALE-1")),
        }
    );
    assert_eq!(
        l.f.client.get_sale(&sale_id),
        crate::Sale {
            sale_id,
            receipt_id: id,
            warehouse_id: l.f.warehouse.clone(),
            weight_kg: 2_000,
            lender: l.lender.clone(),
            loan_ref: String::from_str(&env, LOAN_REF),
            operator: l.operator.clone(),
            previous_owner: l.f.farmer.clone(),
            buyer: buyer.clone(),
            sale_ref: String::from_str(&env, "SALE-1"),
            started_at: DEADLINE + 1,
            completed_at: DEADLINE + 500,
            settlement_ref: None,
            settled_at: None,
        }
    );
    assert_eq!(l.f.client.try_get_lock(&id), Err(Ok(Error::NotLocked)));

    // Indexed under the buyer; still listed under the farmer for history.
    let buyer_ids: std::vec::Vec<u64> =
        l.f.client
            .get_receipts_by_owner(&buyer, &0)
            .iter()
            .map(|r| r.receipt_id)
            .collect();
    assert_eq!(buyer_ids, std::vec![id]);
}

#[test]
fn the_buyer_can_exit_after_a_completed_recovery() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);
    l.complete(&env, id, &buyer);

    // The farmer can no longer sign an exit for it; the buyer can.
    let args = (id, l.f.custodian.clone(), 2_000u64, 1_990u64).into_val(&env);
    sign_only(
        &env,
        &l.f.contract_id,
        "exit",
        args,
        &[&l.f.custodian, &l.f.farmer],
    );
    assert!(l
        .f
        .client
        .try_exit(&id, &l.f.custodian, &2_000, &1_990)
        .is_err());

    env.mock_all_auths();
    l.f.client.exit(&id, &l.f.custodian, &2_000, &1_990);
    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Exited);
    assert_eq!(r.recovery_history.len(), 1, "exit keeps the sale record");
}

#[test]
fn the_buyer_can_pledge_again_and_the_sale_record_survives_later_locks() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    let buyer = Address::generate(&env);
    l.complete(&env, id, &buyer);
    let sale = l.f.client.get_receipt(&id).recovery_history.get(0).unwrap();

    // The buyer pledges part of it to a new lender, who then also defaults
    // them and starts a second recovery, which the buyer then repays.
    let now = DEADLINE + 10;
    env.ledger().set_timestamp(now);
    let lender2 = Address::generate(&env);
    let relocked = l.f.client.lock(
        &id,
        &lender2,
        &1_500,
        &String::from_str(&env, "LOAN-2"),
        &(now + 100),
        &0,
    );
    env.ledger().set_timestamp(now + 101);
    l.f.client.start_recovery(&relocked);
    l.f.client.unlock(&relocked, &1_500);

    let r = l.f.client.get_receipt(&relocked);
    assert_eq!(r.owner, buyer);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.recovery_history.len(), 2);
    assert_eq!(
        r.recovery_history.get(0).unwrap(),
        sale,
        "the first sale is untouched"
    );
    let second = r.recovery_history.get(1).unwrap();
    assert_eq!(second.resolution, RecoveryResolution::Repaid);
    assert_eq!(second.lender, lender2);

    // The unpledged remainder carries the sale record too.
    let remainder = l.f.client.get_receipt(&(relocked + 1));
    assert_eq!(remainder.recovery_history.len(), 1);
    assert_eq!(remainder.recovery_history.get(0).unwrap(), sale);
}

// --- unlock during recovery ------------------------------------------------

#[test]
fn unlock_during_recovery_returns_the_receipt_to_the_owner_not_the_lender() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    env.ledger().set_timestamp(DEADLINE + 50);

    sign_only(
        &env,
        &l.f.contract_id,
        "unlock",
        (id, 2_000u64).into_val(&env),
        &[&l.lender],
    );
    l.f.client.unlock(&id, &2_000);

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.owner, l.f.farmer);
    assert_ne!(r.owner, l.lender);
    assert_eq!(r.lock, MaybeLock::None);
    assert_eq!(r.recovery, MaybeRecovery::None);

    // The aborted recovery stays on the receipt.
    assert_eq!(r.recovery_history.len(), 1);
    let aborted = r.recovery_history.get(0).unwrap();
    assert_eq!(aborted.resolution, RecoveryResolution::Repaid);
    assert_eq!(aborted.started_at, DEADLINE + 1);
    assert_eq!(aborted.resolved_at, Some(DEADLINE + 50));
    assert_eq!(aborted.buyer, None);
}

#[test]
fn unlock_during_recovery_by_the_owner_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);

    sign_only(
        &env,
        &l.f.contract_id,
        "unlock",
        (id, 2_000u64).into_val(&env),
        &[&l.f.farmer],
    );
    assert!(l.f.client.try_unlock(&id, &2_000).is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

#[test]
fn partial_unlock_during_recovery_keeps_the_rest_in_recovery() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);

    let released = l.f.client.unlock(&id, &800);
    let rest = l.f.client.get_receipt(&(released + 1));
    let released = l.f.client.get_receipt(&released);

    assert_eq!(released.status, Status::Active);
    assert_eq!(released.owner, l.f.farmer);
    assert_eq!(released.recovery_history.len(), 1);

    assert_eq!(rest.status, Status::Recovery);
    assert_eq!(rest.weight_kg, 1_200);
    assert!(matches!(rest.recovery, MaybeRecovery::Some(_)));
    assert!(rest.recovery_history.is_empty());

    // The part still in recovery can be sold on its own.
    let buyer = Address::generate(&env);
    l.complete(&env, rest.receipt_id, &buyer);
    assert_eq!(l.f.client.get_receipt(&rest.receipt_id).owner, buyer);
    assert_eq!(
        l.f.client.get_receipt(&released.receipt_id).owner,
        l.f.farmer
    );
}

#[test]
fn exit_and_revoke_on_a_receipt_in_recovery_fail() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);

    for kg in [2_000, 500] {
        assert_eq!(
            l.f.client.try_exit(&id, &l.f.custodian, &kg, &kg),
            Err(Ok(Error::InvalidStatus))
        );
    }
    assert_eq!(
        l.f.client.try_revoke(&id, &String::from_str(&env, "fraud")),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Recovery);
}

// --- events ----------------------------------------------------------------

#[test]
fn recovery_started_and_completed_events_carry_the_indexer_fields() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);

    l.default_and_recover(&env, id);
    let started = crate::events::RecoveryStarted {
        receipt_id: id,
        lender: l.lender.clone(),
        warehouse_id: l.f.warehouse.clone(),
        owner: l.f.farmer.clone(),
        loan_ref: String::from_str(&env, LOAN_REF),
        kg: 2_000,
        due_date: DUE,
        grace_period: GRACE,
        started_at: DEADLINE + 1,
    };
    assert_eq!(
        env.events().all(),
        std::vec![started.to_xdr(&env, &l.f.contract_id)]
    );

    env.ledger().set_timestamp(DEADLINE + 99);
    let buyer = Address::generate(&env);
    let sale_id = l.complete(&env, id, &buyer);
    let completed = crate::events::RecoveryCompleted {
        receipt_id: id,
        lender: l.lender.clone(),
        sale_id,
        operator: l.operator.clone(),
        previous_owner: l.f.farmer.clone(),
        buyer,
        warehouse_id: l.f.warehouse.clone(),
        loan_ref: String::from_str(&env, LOAN_REF),
        kg: 2_000,
        sale_ref: String::from_str(&env, "SALE-1"),
        started_at: DEADLINE + 1,
        completed_at: DEADLINE + 99,
    };
    assert_eq!(
        env.events().all(),
        std::vec![completed.to_xdr(&env, &l.f.contract_id)]
    );
}

#[test]
fn unlock_from_recovery_is_flagged_in_the_event() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let id = locked_receipt(&env, &l);
    l.default_and_recover(&env, id);
    l.f.client.unlock(&id, &2_000);

    let expected = crate::events::Unlocked {
        receipt_id: id,
        lender: l.lender.clone(),
        owner: l.f.farmer.clone(),
        warehouse_id: l.f.warehouse.clone(),
        loan_ref: String::from_str(&env, LOAN_REF),
        kg: 2_000,
        partial: false,
        remaining_receipt_id: None,
        from_recovery: true,
        unlocked_at: DEADLINE + 1,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &l.f.contract_id)]
    );
}
