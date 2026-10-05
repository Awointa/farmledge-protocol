//! `anchor`, `preclaim` and the proof reads. Ordering, storage class and the
//! head formula have their own named tests in `audit.rs`.

use soroban_sdk::{
    testutils::Events as _,
    BytesN, Env, Event as _, Symbol, Vec,
};

use super::{setup, setup_bare, NOW};
use crate::{events, Error, WeightSource, MAX_TREE_DEPTH};

#[test]
fn anchor_records_the_head_and_emits_anchored() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[3; 32]);

    let head = f.client.anchor(&f.custodian, &f.warehouse, &5, &root, &2_048);

    assert_eq!(
        env.events().all(),
        std::vec![events::Anchored {
            warehouse_id: f.warehouse.clone(),
            day: 5,
            leaves: 2_048,
            root,
            prev_head: BytesN::from_array(&env, &[0; 32]),
            head: head.clone(),
            custodian: f.custodian.clone(),
            anchored_at: NOW,
        }
        .to_xdr(&env, &f.contract_id)]
    );
    let stored = f.client.get_anchor(&f.warehouse);
    assert_eq!((stored.day, stored.head), (5, head));
}

/// Days may be skipped (the backend must not, but the contract cannot tell
/// a closed warehouse from a missed day); they may not go backwards.
#[test]
fn anchor_may_skip_days_but_never_go_back() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[3; 32]);
    f.client.anchor(&f.custodian, &f.warehouse, &5, &root, &1);
    f.client.anchor(&f.custodian, &f.warehouse, &9, &root, &1);
    assert_eq!(
        f.client.try_anchor(&f.custodian, &f.warehouse, &8, &root, &1),
        Err(Ok(Error::AnchorOutOfOrder))
    );
}

/// Each warehouse has its own chain.
#[test]
fn warehouses_are_anchored_independently() {
    let env = Env::default();
    let f = setup(&env);
    let other = Symbol::new(&env, "WH_ZARIA_01");
    f.client.add_warehouse(&other);
    f.client.add_custodian(&other, &f.custodian);
    let root = BytesN::from_array(&env, &[3; 32]);

    f.client.anchor(&f.custodian, &f.warehouse, &9, &root, &1);
    f.client.anchor(&f.custodian, &other, &2, &root, &1);
    assert_eq!(f.client.get_anchor(&f.warehouse).day, 9);
    assert_eq!(f.client.get_anchor(&other).day, 2);
}

#[test]
fn anchor_rejects_an_empty_or_oversized_tree() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[3; 32]);
    for leaves in [0u32, (1 << MAX_TREE_DEPTH) + 1, u32::MAX] {
        assert_eq!(
            f.client.try_anchor(&f.custodian, &f.warehouse, &1, &root, &leaves),
            Err(Ok(Error::InvalidLeafCount))
        );
    }
    f.client.anchor(&f.custodian, &f.warehouse, &1, &root, &(1 << MAX_TREE_DEPTH));
}

#[test]
fn anchor_rejects_an_unregistered_warehouse() {
    let env = Env::default();
    let f = setup_bare(&env);
    assert_eq!(
        f.client.try_anchor(&f.custodian, &f.warehouse, &1, &BytesN::from_array(&env, &[3; 32]), &1),
        Err(Ok(Error::WarehouseNotRegistered))
    );
}

/// A rejected anchor changes nothing.
#[test]
fn a_rejected_anchor_leaves_the_head_alone() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[3; 32]);
    let head = f.client.anchor(&f.custodian, &f.warehouse, &5, &root, &1);
    let _ = f.client.try_anchor(&f.custodian, &f.warehouse, &5, &root, &1);
    let _ = f.client.try_anchor(&f.custodian, &f.warehouse, &u32::MAX, &root, &1);
    let stored = f.client.get_anchor(&f.warehouse);
    assert_eq!((stored.day, stored.head), (5, head));
}

// --- reads -------------------------------------------------------------------

#[test]
fn verify_leaf_checks_any_stored_day_and_any_kind() {
    let env = Env::default();
    let f = setup(&env);
    let mut d1 = [
        f.balance_leaf(&env, "DEP-1", 1_000, 10, WeightSource::Scale),
        f.balance_leaf(&env, "DEP-2", 1_000, 10, WeightSource::Scale),
    ];
    d1[1].kind = crate::KIND_DEPOSIT;
    let p1 = f.anchor_next_day(&env, &mut d1);
    let mut d2 = [f.balance_leaf(&env, "DEP-3", 1_000, 10, WeightSource::Scale)];
    let p2 = f.anchor_next_day(&env, &mut d2);

    assert!(f.client.verify_leaf(&d1[0], &p1[0]), "an earlier day, while stored");
    assert!(f.client.verify_leaf(&d1[1], &p1[1]), "a deposit leaf is still in the books");
    assert!(f.client.verify_leaf(&d2[0], &p2[0]));

    let mut wrong = d1[0].clone();
    wrong.kg += 1;
    assert!(!f.client.verify_leaf(&wrong, &p1[0]));
    assert!(!f.client.verify_leaf(&d1[0], &p1[1]));
    let mut unanchored = d1[0].clone();
    unanchored.day = 999;
    assert!(!f.client.verify_leaf(&unanchored, &p1[0]));
}

#[test]
fn get_anchor_before_the_first_anchor_is_no_anchor() {
    let env = Env::default();
    let f = setup(&env);
    assert_eq!(f.client.try_get_anchor(&f.warehouse), Err(Ok(Error::NoAnchor)));
}

// --- preclaim ----------------------------------------------------------------

#[test]
fn preclaim_emits_one_event_per_number() {
    let env = Env::default();
    let f = setup(&env);
    f.client.preclaim(&f.warehouse, &Vec::from_array(&env, [4u64, 2_000]));

    let expected: std::vec::Vec<_> = [4u64, 2_000]
        .iter()
        .map(|n| {
            events::Preclaimed {
                warehouse_id: f.warehouse.clone(),
                deposit_no: *n,
                claimed_at: NOW,
            }
            .to_xdr(&env, &f.contract_id)
        })
        .collect();
    assert_eq!(env.events().all(), expected);
}

#[test]
fn preclaim_takes_at_most_25_numbers() {
    let env = Env::default();
    let f = setup(&env);
    let mut nos = Vec::new(&env);
    for n in 0..26u64 {
        nos.push_back(n);
    }
    assert_eq!(f.client.try_preclaim(&f.warehouse, &nos), Err(Ok(Error::BatchTooLarge)));
    nos.pop_back();
    assert_eq!(f.client.preclaim(&f.warehouse, &nos), 25);
}

#[test]
fn preclaim_rejects_an_unregistered_warehouse() {
    let env = Env::default();
    let f = setup(&env);
    assert_eq!(
        f.client.try_preclaim(&Symbol::new(&env, "WH_NOWHERE"), &Vec::from_array(&env, [1u64])),
        Err(Ok(Error::WarehouseNotRegistered))
    );
}

/// A number repeated within one call is caught like any other repeat.
#[test]
fn preclaim_rejects_a_number_repeated_in_one_call() {
    let env = Env::default();
    let f = setup(&env);
    assert_eq!(
        f.client.try_preclaim(&f.warehouse, &Vec::from_array(&env, [7u64, 7])),
        Err(Ok(Error::DuplicateDepositId))
    );
    assert!(!f.client.is_claimed(&f.warehouse, &7));
}

/// Pre-claims and mints share one guard: a minted number cannot be
/// pre-claimed either.
#[test]
fn a_minted_number_cannot_be_preclaimed() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-1");
    let no = f.client.get_receipt(&id).deposit_no;
    assert_eq!(
        f.client.try_preclaim(&f.warehouse, &Vec::from_array(&env, [no])),
        Err(Ok(Error::DuplicateDepositId))
    );
}
