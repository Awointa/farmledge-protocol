use soroban_sdk::{testutils::Address as _, Address, Env, IntoVal, String};

use super::{live_weight, next_id, setup_lending, sign_only, Lending, DUE, GRACE, LOAN_REF, NOW};
use crate::{Error, Lock, MaybeLock, Status};

fn lock_args(env: &Env, l: &Lending, id: u64, kg: u64) -> soroban_sdk::Vec<soroban_sdk::Val> {
    (
        id,
        l.lender.clone(),
        kg,
        String::from_str(env, LOAN_REF),
        DUE,
        GRACE,
    )
        .into_val(env)
}

fn try_lock(env: &Env, l: &Lending, id: u64, kg: u64) -> Result<u64, Error> {
    match l.f.client.try_lock(
        &id,
        &l.lender,
        &kg,
        &String::from_str(env, LOAN_REF),
        &DUE,
        &GRACE,
    ) {
        Ok(Ok(v)) => Ok(v),
        Err(Ok(e)) => Err(e),
        other => panic!("unexpected host failure: {other:?}"),
    }
}

// --- authorisation ---------------------------------------------------------

#[test]
fn lock_fails_with_only_the_lenders_signature() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    let args = lock_args(&env, &l, id, 2_000);
    sign_only(&env, &l.f.contract_id, "lock", args, &[&l.lender]);

    assert!(l
        .f
        .client
        .try_lock(
            &id,
            &l.lender,
            &2_000,
            &String::from_str(&env, LOAN_REF),
            &DUE,
            &GRACE
        )
        .is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);
}

#[test]
fn lock_fails_with_only_the_owners_signature() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    let args = lock_args(&env, &l, id, 2_000);
    sign_only(&env, &l.f.contract_id, "lock", args, &[&l.f.farmer]);

    assert!(l
        .f
        .client
        .try_lock(
            &id,
            &l.lender,
            &2_000,
            &String::from_str(&env, LOAN_REF),
            &DUE,
            &GRACE
        )
        .is_err());
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);
}

/// The control for the two tests above: the same call with both signatures.
#[test]
fn lock_succeeds_with_both_signatures() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    let args = lock_args(&env, &l, id, 2_000);
    sign_only(
        &env,
        &l.f.contract_id,
        "lock",
        args,
        &[&l.lender, &l.f.farmer],
    );

    l.f.client.lock(
        &id,
        &l.lender,
        &2_000,
        &String::from_str(&env, LOAN_REF),
        &DUE,
        &GRACE,
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn a_receipt_cannot_be_pledged_to_its_own_owner() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    assert_eq!(
        l.f.client.try_lock(
            &id,
            &l.f.farmer,
            &2_000,
            &String::from_str(&env, LOAN_REF),
            &DUE,
            &GRACE
        ),
        Err(Ok(Error::LenderIsOwner))
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);
}

// --- state -----------------------------------------------------------------

#[test]
fn full_lock_pledges_the_receipt_in_place() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    let locked = l.lock(&env, id, 2_000);
    assert_eq!(locked, id, "a full lock creates no child");

    let r = l.f.client.get_receipt(&id);
    let expected = Lock {
        lender: l.lender.clone(),
        loan_ref: String::from_str(&env, LOAN_REF),
        due_date: DUE,
        grace_period: GRACE,
    };
    assert_eq!(r.status, Status::Locked);
    assert_eq!(r.owner, l.f.farmer, "pledging does not change ownership");
    assert_eq!(r.lock, MaybeLock::Some(expected.clone()));
    assert_eq!(l.f.client.get_lock(&id), expected);
}

#[test]
fn partial_lock_splits_and_leaves_the_remainder_active_under_the_owner() {
    let env = Env::default();
    let l = setup_lending(&env);
    let parent = l.f.mint(&env, "DEP-1");

    let locked_id = l.lock(&env, parent, 600);
    let locked = l.f.client.get_receipt(&locked_id);
    let remainder = l.f.client.get_receipt(&(locked_id + 1));

    assert_eq!(l.f.client.get_receipt(&parent).status, Status::Split);
    assert_eq!(locked.weight_kg, 600);
    assert_eq!(locked.status, Status::Locked);
    assert_eq!(remainder.weight_kg, 1_400);
    assert_eq!(remainder.status, Status::Active);
    assert_eq!(remainder.lock, MaybeLock::None);
    assert_eq!(remainder.owner, l.f.farmer);
    assert_eq!(live_weight(&l.f, next_id(&l.f) - 1), 2_000);
}

#[test]
fn lock_on_an_already_locked_receipt_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    assert_eq!(try_lock(&env, &l, id, 2_000), Err(Error::InvalidStatus));
    assert_eq!(try_lock(&env, &l, id, 500), Err(Error::InvalidStatus));

    // Nor can a different lender take the same grain.
    let other = Address::generate(&env);
    assert_eq!(
        l.f.client.try_lock(
            &id,
            &other,
            &2_000,
            &String::from_str(&env, "LOAN-2"),
            &DUE,
            &GRACE
        ),
        Err(Ok(Error::InvalidStatus))
    );
    assert_eq!(l.f.client.get_lock(&id).lender, l.lender);
}

#[test]
fn lock_on_a_bag_estimate_receipt_fails_and_on_a_scale_receipt_succeeds() {
    let env = Env::default();
    let l = setup_lending(&env);
    let bags = l.f.mint_bag_estimate(&env, "DEP-BAGS");
    let scale = l.f.mint(&env, "DEP-SCALE");

    assert_eq!(
        try_lock(&env, &l, bags, 2_000),
        Err(Error::OnlyScaleEligibleForLock)
    );
    // Partial is no loophole: the check runs before any split.
    assert_eq!(
        try_lock(&env, &l, bags, 500),
        Err(Error::OnlyScaleEligibleForLock)
    );
    assert_eq!(l.f.client.get_receipt(&bags).status, Status::Active);

    assert_eq!(try_lock(&env, &l, scale, 2_000), Ok(scale));
}

#[test]
fn lock_rejects_exited_revoked_and_split_receipts() {
    let env = Env::default();
    let l = setup_lending(&env);

    let exited = l.f.mint(&env, "DEP-EXIT");
    l.f.client.exit(&exited, &l.f.custodian, &2_000, &2_000);
    assert_eq!(try_lock(&env, &l, exited, 2_000), Err(Error::InvalidStatus));

    let revoked = l.f.mint(&env, "DEP-REV");
    l.f.client
        .revoke(&revoked, &String::from_str(&env, "fraud"));
    assert_eq!(
        try_lock(&env, &l, revoked, 2_000),
        Err(Error::InvalidStatus)
    );

    let parent = l.f.mint(&env, "DEP-SPLIT");
    l.f.client.exit(&parent, &l.f.custodian, &500, &500);
    assert_eq!(try_lock(&env, &l, parent, 1_000), Err(Error::InvalidStatus));
}

#[test]
fn lock_rejects_impossible_quantities() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    assert_eq!(try_lock(&env, &l, id, 0), Err(Error::InvalidSplitAmount));
    assert_eq!(
        try_lock(&env, &l, id, 2_001),
        Err(Error::InvalidSplitAmount)
    );
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Active);
    assert_eq!(next_id(&l.f), id + 1, "no split happened");
}

#[test]
fn lock_rejects_a_due_date_that_is_not_in_the_future_or_that_overflows() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    let loan = String::from_str(&env, LOAN_REF);

    for due in [NOW - 1, NOW] {
        assert_eq!(
            l.f.client
                .try_lock(&id, &l.lender, &2_000, &loan, &due, &GRACE),
            Err(Ok(Error::InvalidDueDate)),
            "due {due}"
        );
    }
    assert_eq!(
        l.f.client
            .try_lock(&id, &l.lender, &2_000, &loan, &(u64::MAX - 10), &11),
        Err(Ok(Error::InvalidDueDate)),
        "due + grace overflows"
    );

    // One second ahead with no grace is valid: ordering, not policy.
    l.f.client
        .lock(&id, &l.lender, &2_000, &loan, &(NOW + 1), &0);
}

#[test]
fn exit_on_a_locked_receipt_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    for kg in [2_000, 500] {
        assert_eq!(
            l.f.client.try_exit(&id, &l.f.custodian, &kg, &kg),
            Err(Ok(Error::InvalidStatus))
        );
    }
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Locked);
}

#[test]
fn revoke_from_locked_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    assert_eq!(
        l.f.client.try_revoke(&id, &String::from_str(&env, "fraud")),
        Err(Ok(Error::InvalidStatus))
    );

    // The supported path: the lender unlocks, then the admin revokes.
    l.f.client.unlock(&id, &2_000);
    l.f.client.revoke(&id, &String::from_str(&env, "fraud"));
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Revoked);
}

// --- reads -----------------------------------------------------------------

#[test]
fn is_pledgeable_reflects_status_and_weight_source() {
    let env = Env::default();
    let l = setup_lending(&env);
    let scale = l.f.mint(&env, "DEP-SCALE");
    let bags = l.f.mint_bag_estimate(&env, "DEP-BAGS");

    assert!(l.f.client.is_pledgeable(&scale));
    assert!(!l.f.client.is_pledgeable(&bags));

    l.lock(&env, scale, 2_000);
    assert!(!l.f.client.is_pledgeable(&scale), "already pledged");

    assert_eq!(
        l.f.client.try_is_pledgeable(&999),
        Err(Ok(Error::ReceiptNotFound))
    );
}

#[test]
fn get_lock_is_not_locked_for_a_free_or_split_receipt() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");

    assert_eq!(l.f.client.try_get_lock(&id), Err(Ok(Error::NotLocked)));

    // A locked receipt that is then partly released is retired as `Split`
    // still carrying its old lock. That is history, not a live pledge.
    l.lock(&env, id, 2_000);
    l.f.client.unlock(&id, &500);
    assert_eq!(l.f.client.get_receipt(&id).status, Status::Split);
    assert_eq!(l.f.client.try_get_lock(&id), Err(Ok(Error::NotLocked)));
}

#[test]
fn locked_receipts_are_indexed_under_the_lender() {
    let env = Env::default();
    let l = setup_lending(&env);
    let a = l.f.mint(&env, "DEP-A");
    let b = l.f.mint(&env, "DEP-B");

    let locked_a = l.lock(&env, a, 2_000);
    let locked_b = l.lock(&env, b, 800);
    // Partially releasing b's pledge splits it; the part that stays locked is
    // a new receipt and must show up under the lender too.
    let released = l.f.client.unlock(&locked_b, &300);
    let still_locked = released + 1;

    assert_eq!(l.f.client.lender_page_count(&l.lender), 1);
    let ids: std::vec::Vec<u64> =
        l.f.client
            .get_receipts_by_lender(&l.lender, &0)
            .iter()
            .map(|r| r.receipt_id)
            .collect();
    for id in [locked_a, locked_b, released, still_locked] {
        assert!(ids.contains(&id), "{id} missing from {ids:?}");
    }

    let live: std::vec::Vec<u64> =
        l.f.client
            .get_receipts_by_lender(&l.lender, &0)
            .iter()
            .filter(|r| r.status == Status::Locked)
            .map(|r| r.receipt_id)
            .collect();
    assert_eq!(live, std::vec![locked_a, still_locked]);

    let stranger = Address::generate(&env);
    assert_eq!(l.f.client.lender_page_count(&stranger), 0);
    assert!(l.f.client.get_receipts_by_lender(&stranger, &0).is_empty());
}

#[test]
fn full_lock_emits_one_locked_event_with_the_indexer_fields() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);

    let expected = crate::events::Locked {
        receipt_id: id,
        lender: l.lender.clone(),
        owner: l.f.farmer.clone(),
        kg: 2_000,
        loan_ref: String::from_str(&env, LOAN_REF),
        due_date: DUE,
        grace_period: GRACE,
        locked_at: NOW,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &l.f.contract_id)]
    );
}

#[test]
fn partial_lock_emits_split_then_locked() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let parent = l.f.mint(&env, "DEP-1");
    let locked = l.lock(&env, parent, 600);

    let split = crate::events::Split {
        parent_id: parent,
        taken_id: locked,
        taken_weight_kg: 600,
        remainder_id: locked + 1,
        remainder_weight_kg: 1_400,
    };
    let lock = crate::events::Locked {
        receipt_id: locked,
        lender: l.lender.clone(),
        owner: l.f.farmer.clone(),
        kg: 600,
        loan_ref: String::from_str(&env, LOAN_REF),
        due_date: DUE,
        grace_period: GRACE,
        locked_at: NOW,
    };
    assert_eq!(
        env.events().all(),
        std::vec![
            split.to_xdr(&env, &l.f.contract_id),
            lock.to_xdr(&env, &l.f.contract_id),
        ]
    );
}
