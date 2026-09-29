use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, IntoVal, String,
};

use super::{live_weight, next_id, setup_lending, sign_only, DUE, GRACE, NOW};
use crate::{Error, MaybeLock, MaybeRecovery, Status};

// --- authorisation ---------------------------------------------------------

#[test]
fn unlock_by_the_owner_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    sign_only(
        &env,
        &l.f.contract_id,
        "unlock",
        (id, 2_000u64).into_val(&env),
        &[&l.f.farmer],
    );
    assert!(l.f.client.try_unlock(&id, &2_000).is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn unlock_by_a_different_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    let other_lender = Address::generate(&env);
    sign_only(
        &env,
        &l.f.contract_id,
        "unlock",
        (id, 2_000u64).into_val(&env),
        &[&other_lender],
    );
    assert!(l.f.client.try_unlock(&id, &2_000).is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn unlock_by_the_recorded_lender_succeeds() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

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
    assert_eq!(r.lock, MaybeLock::None);
    assert_eq!(r.owner, l.f.farmer);
}

#[test]
fn extend_due_by_the_owner_or_another_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    let other_lender = Address::generate(&env);

    for signer in [&l.f.farmer, &other_lender] {
        sign_only(
            &env,
            &l.f.contract_id,
            "extend_due",
            (id, DUE + 1).into_val(&env),
            &[signer],
        );
        assert!(l.f.client.try_extend_due(&id, &(DUE + 1)).is_err());
    }
    assert_eq!(l.f.client.get_lock(&id).due_date, DUE);
}

// --- state -----------------------------------------------------------------

#[test]
fn unlock_rejects_a_receipt_that_is_not_pledged() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    assert_eq!(
        l.f.client.try_unlock(&id, &2_000),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(
        l.f.client.try_unlock(&999, &1),
        Err(Ok(Error::ReceiptNotFound))
    );
}

#[test]
fn unlock_rejects_impossible_quantities() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    for kg in [0, 2_001] {
        assert_eq!(
            l.f.client.try_unlock(&id, &kg),
            Err(Ok(Error::InvalidSplitAmount))
        );
    }
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn partial_unlock_releases_a_portion_and_keeps_the_rest_locked() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    let released_id = l.f.client.unlock(&id, &700);
    let released = l.f.client.get_receipt(&released_id);
    let remaining = l.f.client.get_receipt(&(released_id + 1));

    assert_eq!(released.weight_kg, 700);
    assert_eq!(released.status, Status::Active);
    assert_eq!(released.lock, MaybeLock::None);
    assert_eq!(released.owner, l.f.farmer);

    assert_eq!(remaining.weight_kg, 1_300);
    assert_eq!(remaining.status, Status::Locked);
    assert_eq!(
        l.f.client.get_lock(&remaining.receipt_id),
        l.f.client.get_receipt(&id).lock.unwrap_lock(),
        "the remaining portion keeps the original terms"
    );
}

#[test]
fn extend_due_to_an_earlier_or_equal_date_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    for date in [DUE - 1, DUE] {
        assert_eq!(
            l.f.client.try_extend_due(&id, &date),
            Err(Ok(Error::DueDateNotLater))
        );
    }
    assert_eq!(
        l.f.client.try_extend_due(&id, &u64::MAX),
        Err(Ok(Error::InvalidDueDate)),
        "new due + grace overflows"
    );

    l.f.client.extend_due(&id, &(DUE + 1));
    assert_eq!(l.f.client.get_lock(&id).due_date, DUE + 1);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn extend_due_rejects_a_receipt_that_is_not_locked() {
    let env = Env::default();
    let l = setup_lending(&env);
    let free = l.f.mint(&env, "DEP-FREE");
    assert_eq!(
        l.f.client.try_extend_due(&free, &(DUE + 1)),
        Err(Ok(Error::InvalidStatus))
    );

    let rec = l.f.mint(&env, "DEP-REC");
    l.lock(&env, rec, 2_000);
    l.default_and_recover(&env, rec);
    assert_eq!(
        l.f.client.try_extend_due(&rec, &(DUE * 2)),
        Err(Ok(Error::InvalidStatus)),
        "once recovery starts the due date is history"
    );
}

#[test]
fn unlocked_event_marks_a_partial_release() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    let released = l.f.client.unlock(&id, &700);

    let split = crate::events::Split {
        parent_id: id,
        taken_id: released,
        taken_weight_kg: 700,
        remainder_id: released + 1,
        remainder_weight_kg: 1_300,
    };
    let unlocked = crate::events::Unlocked {
        receipt_id: released,
        lender: l.lender.clone(),
        owner: l.f.farmer.clone(),
        kg: 700,
        partial: true,
        remaining_receipt_id: Some(released + 1),
        from_recovery: false,
        unlocked_at: NOW,
    };
    assert_eq!(
        env.events().all(),
        std::vec![
            split.to_xdr(&env, &l.f.contract_id),
            unlocked.to_xdr(&env, &l.f.contract_id),
        ]
    );
}

#[test]
fn due_extended_event_carries_old_and_new_dates() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.f.client.extend_due(&id, &(DUE + 86_400));

    let expected = crate::events::DueExtended {
        receipt_id: id,
        lender: l.lender.clone(),
        kg: 2_000,
        old_due_date: DUE,
        new_due_date: DUE + 86_400,
        extended_at: NOW,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &l.f.contract_id)]
    );
}

// --- arithmetic ------------------------------------------------------------

#[test]
fn partial_lock_then_partial_unlock_then_partial_lock_conserves_weight() {
    let env = Env::default();
    let l = setup_lending(&env);
    let root = l.f.mint(&env, "DEP-1"); // 2,000kg

    let locked = l.lock(&env, root, 1_200);
    let free_a = locked + 1; // 800 free
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);

    let released = l.f.client.unlock(&locked, &500);
    let still_locked = released + 1; // 700 locked
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);

    let relocked = l.lock(&env, released, 200);
    let free_b = relocked + 1; // 300 free
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);

    let weight = |id: u64| l.f.client.get_receipt(&id).weight_kg;
    let status = |id: u64| l.f.client.get_receipt(&id).status;
    assert_eq!(
        (
            weight(free_a),
            weight(still_locked),
            weight(relocked),
            weight(free_b)
        ),
        (800, 700, 200, 300)
    );
    assert_eq!(
        weight(free_a) + weight(still_locked) + weight(relocked) + weight(free_b),
        2_000
    );
    assert_eq!(status(free_a), Status::Active);
    assert_eq!(status(still_locked), Status::Locked);
    assert_eq!(status(relocked), Status::Locked);
    assert_eq!(status(free_b), Status::Active);
}

// --- regression: a locked portion and its free remainder are independent --

#[test]
fn acting_on_the_free_remainder_never_touches_the_locked_portion() {
    let env = Env::default();
    let l = setup_lending(&env);
    let root = l.f.mint(&env, "DEP-1");
    let locked = l.lock(&env, root, 1_200);
    let free = locked + 1;
    let before = l.f.client.get_receipt(&locked);

    // Exit part of the free remainder, pledge another part to a second
    // lender, then exit what is left.
    let second_lender = Address::generate(&env);
    let exited = l.f.client.exit(&free, &l.f.custodian, &300, &300);
    let free2 = exited + 1;
    let other_lock = l.f.client.lock(
        &free2,
        &second_lender,
        &200,
        &String::from_str(&env, "LOAN-B"),
        &DUE,
        &0,
    );
    l.f.client
        .exit(&(other_lock + 1), &l.f.custodian, &300, &300);

    assert_eq!(l.f.client.get_receipt(&locked), before);
    assert_eq!(l.f.client.get_lock(&other_lock).lender, second_lender);
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);
}

#[test]
fn acting_on_the_locked_portion_never_touches_the_free_remainder() {
    let env = Env::default();
    let l = setup_lending(&env);
    let root = l.f.mint(&env, "DEP-1");
    let locked = l.lock(&env, root, 1_200);
    let free = locked + 1;
    let before = l.f.client.get_receipt(&free);

    l.f.client.extend_due(&locked, &(DUE + 1));
    let released = l.f.client.unlock(&locked, &200);
    // The extension pushed the deadline out by one second.
    env.ledger().set_timestamp(DUE + 1 + GRACE + 1);
    l.f.client.start_recovery(&(released + 1));
    let buyer = Address::generate(&env);
    l.complete(&env, released + 1, &buyer);

    assert_eq!(l.f.client.get_receipt(&free), before);
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);
}

/// `MaybeLock` has no accessor in the contract; tests want one.
trait UnwrapLock {
    fn unwrap_lock(self) -> crate::Lock;
}

impl UnwrapLock for MaybeLock {
    fn unwrap_lock(self) -> crate::Lock {
        match self {
            MaybeLock::Some(lock) => lock,
            MaybeLock::None => panic!("receipt is not locked"),
        }
    }
}

#[test]
fn a_full_unlock_leaves_no_recovery_trace_when_there_was_no_recovery() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.f.client.unlock(&id, &2_000);

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.recovery, MaybeRecovery::None);
    assert!(r.recovery_history.is_empty());
}
