use soroban_sdk::{
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, String, Symbol,
};

use super::{live_weight, setup, Fixture};
use crate::{Error, Status};

#[test]
fn full_exit_retires_the_receipt_and_records_exit_weight() {
    let env = Env::default();
    let f = setup(&env);
    env.ledger().set_timestamp(1_700_000_000);

    let id = f.mint(&env, "DEP-0001");
    // 2,000kg on the books, 1,985kg on the scale on the way out.
    let exited_id = f.client.exit(&id, &f.custodian, &2_000, &1_985);

    assert_eq!(exited_id, id, "a full exit creates no child");

    let r = f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Exited);
    assert_eq!(r.weight_kg, 2_000, "book weight is unchanged by shrinkage");
    assert_eq!(r.exit_weight_kg, Some(1_985));
    assert_eq!(r.exit_date, Some(1_700_000_000));
    assert_eq!(r.parent_id, None);
}

#[test]
fn partial_exit_splits_and_the_remainder_stays_active() {
    let env = Env::default();
    let f = setup(&env);

    let parent_id = f.mint(&env, "DEP-0001"); // 2,000kg, 20 bags
    let exited_id = f.client.exit(&parent_id, &f.custodian, &500, &498);

    let parent = f.client.get_receipt(&parent_id);
    let exited = f.client.get_receipt(&exited_id);
    // The split allocates `taken` first, then `remainder`.
    let remainder = f.client.get_receipt(&(exited_id + 1));

    // Parent is retired and never touched again.
    assert_eq!(parent.status, Status::Split);
    assert_eq!(parent.weight_kg, 2_000);
    assert_eq!(parent.exit_weight_kg, None);

    // Children sum to the parent, exactly.
    assert_eq!(exited.weight_kg + remainder.weight_kg, parent.weight_kg);
    assert_eq!(exited.weight_kg, 500);
    assert_eq!(remainder.weight_kg, 1_500);

    // Both carry lineage.
    assert_eq!(exited.parent_id, Some(parent_id));
    assert_eq!(remainder.parent_id, Some(parent_id));

    // The exited portion is retired with its real weight; the rest stays live.
    assert_eq!(exited.status, Status::Exited);
    assert_eq!(exited.exit_weight_kg, Some(498));
    assert_eq!(remainder.status, Status::Active);
    assert_eq!(remainder.exit_weight_kg, None);
    assert_eq!(remainder.exit_date, None);

    // Identity is inherited: same deposit, same owner, same attesters.
    assert_eq!(remainder.deposit_id, String::from_str(&env, "DEP-0001"));
    assert_eq!(remainder.owner, parent.owner);
    assert_eq!(remainder.custodian, parent.custodian);
    assert_eq!(remainder.verifier, parent.verifier);
    assert_eq!(remainder.commodity, parent.commodity);
    assert_eq!(remainder.deposit_date, parent.deposit_date);
}

#[test]
fn partial_exit_prorates_bag_count_without_losing_a_bag() {
    let env = Env::default();
    let f = setup(&env);

    let parent_id = f.mint_weighing(&env, "DEP-BAGS", 2_000, 20);
    let exited_id = f.client.exit(&parent_id, &f.custodian, &500, &500);

    let exited = f.client.get_receipt(&exited_id);
    let remainder = f.client.get_receipt(&(exited_id + 1));

    assert_eq!(exited.bag_count, 5, "500/2000 of 20 bags");
    assert_eq!(remainder.bag_count, 15);
    assert_eq!(exited.bag_count + remainder.bag_count, 20);
}

/// A withdrawal too small to account for a whole bag rounds to zero bags and
/// gives every bag to the remainder. Weight, not bag count, is authoritative.
#[test]
fn a_sub_bag_withdrawal_rounds_bags_to_zero_and_still_conserves_them() {
    let env = Env::default();
    let f = setup(&env);

    let parent_id = f.mint_weighing(&env, "DEP-TINY", 2_000, 20);
    let exited_id = f.client.exit(&parent_id, &f.custodian, &1, &1);

    let exited = f.client.get_receipt(&exited_id);
    let remainder = f.client.get_receipt(&(exited_id + 1));

    assert_eq!(exited.bag_count, 0);
    assert_eq!(remainder.bag_count, 20);
    assert_eq!(exited.weight_kg + remainder.weight_kg, 2_000);
}

#[test]
fn repeated_partial_exits_conserve_total_weight() {
    let env = Env::default();
    let f = setup(&env);

    let mut active = f.mint_weighing(&env, "DEP-SEQ", 10_000, 100);

    for kg in [1_000u64, 2_500, 750, 3_000] {
        f.client.exit(&active, &f.custodian, &kg, &kg);
        // The remainder is always the id after the exited child.
        active += 2;
    }

    let remaining = f.client.get_receipt(&active);
    assert_eq!(remaining.status, Status::Active);
    assert_eq!(remaining.weight_kg, 10_000 - (1_000 + 2_500 + 750 + 3_000));
    assert_eq!(live_weight(&f, active), 10_000);
}

#[test]
fn split_children_are_indexed_for_owner_and_warehouse() {
    let env = Env::default();
    let f = setup(&env);

    let parent_id = f.mint(&env, "DEP-IDX");
    f.client.exit(&parent_id, &f.custodian, &500, &500);

    // parent + two children
    assert_eq!(f.client.get_receipts_by_owner(&f.farmer, &0).len(), 3);
    assert_eq!(
        f.client.get_receipts_by_warehouse(&f.warehouse, &0).len(),
        3
    );
}

// --- authorisation ---------------------------------------------------------

fn exit_args(
    env: &Env,
    f: &Fixture,
    id: u64,
    kg: u64,
    weight: u64,
) -> soroban_sdk::Vec<soroban_sdk::Val> {
    (id, f.custodian.clone(), kg, weight).into_val(env)
}

#[test]
fn exit_fails_with_only_the_custodian_signature() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    env.mock_auths(&[MockAuth {
        address: &f.custodian,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "exit",
            args: exit_args(&env, &f, id, 2_000, 2_000),
            sub_invokes: &[],
        },
    }]);

    assert!(f
        .client
        .try_exit(&id, &f.custodian, &2_000, &2_000)
        .is_err());
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);
}

#[test]
fn exit_fails_with_only_the_owner_signature() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    env.mock_auths(&[MockAuth {
        address: &f.farmer,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "exit",
            args: exit_args(&env, &f, id, 2_000, 2_000),
            sub_invokes: &[],
        },
    }]);

    assert!(f
        .client
        .try_exit(&id, &f.custodian, &2_000, &2_000)
        .is_err());
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);
}

#[test]
fn exit_succeeds_with_both_signatures() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");
    let args = exit_args(&env, &f, id, 2_000, 2_000);

    env.mock_auths(&[
        MockAuth {
            address: &f.custodian,
            invoke: &MockAuthInvoke {
                contract: &f.contract_id,
                fn_name: "exit",
                args: args.clone(),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &f.farmer,
            invoke: &MockAuthInvoke {
                contract: &f.contract_id,
                fn_name: "exit",
                args,
                sub_invokes: &[],
            },
        },
    ]);

    f.client.exit(&id, &f.custodian, &2_000, &2_000);
    assert_eq!(f.client.get_receipt(&id).status, Status::Exited);
}

/// Any custodian registered for the warehouse may release grain — the night
/// shift is not blocked because the day shift signed the intake.
#[test]
fn a_different_custodian_of_the_same_warehouse_may_sign_the_exit() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    let night_shift = Address::generate(&env);
    f.client.add_custodian(&f.warehouse, &night_shift);

    f.client.exit(&id, &night_shift, &2_000, &2_000);
    assert_eq!(f.client.get_receipt(&id).status, Status::Exited);
}

#[test]
fn exit_rejects_a_custodian_from_another_warehouse() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    let other_warehouse = Symbol::new(&env, "WH_KADUNA_02");
    let outsider = Address::generate(&env);
    f.client.add_warehouse(&other_warehouse);
    f.client.add_custodian(&other_warehouse, &outsider);

    assert_eq!(
        f.client.try_exit(&id, &outsider, &2_000, &2_000),
        Err(Ok(Error::CustodianNotRegistered))
    );
}

// --- validation ------------------------------------------------------------

#[test]
fn exit_rejects_anything_not_active() {
    let env = Env::default();
    let f = setup(&env);

    // Already exited.
    let exited = f.mint(&env, "DEP-EXITED");
    f.client.exit(&exited, &f.custodian, &2_000, &2_000);
    assert_eq!(
        f.client.try_exit(&exited, &f.custodian, &1, &1),
        Err(Ok(Error::InvalidStatus))
    );

    // Revoked.
    let revoked = f.mint(&env, "DEP-REVOKED");
    f.client
        .revoke(&revoked, &String::from_str(&env, "fraudulent intake"));
    assert_eq!(
        f.client.try_exit(&revoked, &f.custodian, &1, &1),
        Err(Ok(Error::InvalidStatus))
    );

    // A retired split parent.
    let parent = f.mint(&env, "DEP-SPLIT");
    f.client.exit(&parent, &f.custodian, &500, &500);
    assert_eq!(f.client.get_receipt(&parent).status, Status::Split);
    assert_eq!(
        f.client.try_exit(&parent, &f.custodian, &1, &1),
        Err(Ok(Error::InvalidStatus))
    );
}

#[test]
fn exit_rejects_impossible_quantities() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001"); // 2,000kg

    assert_eq!(
        f.client.try_exit(&id, &f.custodian, &2_001, &2_001),
        Err(Ok(Error::InvalidSplitAmount)),
        "more than the receipt holds"
    );
    assert_eq!(
        f.client.try_exit(&id, &f.custodian, &0, &1),
        Err(Ok(Error::InvalidSplitAmount)),
        "nothing at all"
    );
    assert_eq!(
        f.client.try_exit(&id, &f.custodian, &500, &0),
        Err(Ok(Error::InvalidExitWeight)),
        "weighed zero on the way out"
    );

    // None of the rejections left a mark.
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);
    assert_eq!(f.client.owner_page_count(&f.farmer), 1);
    assert_eq!(f.client.get_receipts_by_owner(&f.farmer, &0).len(), 1);
}

#[test]
fn exit_rejects_an_unknown_receipt() {
    let env = Env::default();
    let f = setup(&env);

    assert_eq!(
        f.client.try_exit(&999, &f.custodian, &1, &1),
        Err(Ok(Error::ReceiptNotFound))
    );
}

#[test]
fn partial_exit_emits_split_then_exited() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let parent_id = f.mint(&env, "DEP-EVT");
    let exited_id = f.client.exit(&parent_id, &f.custodian, &500, &498);

    let split = crate::events::Split {
        parent_id,
        owner: f.farmer.clone(),
        warehouse_id: f.warehouse.clone(),
        status: Status::Active,
        taken_id: exited_id,
        taken_weight_kg: 500,
        taken_bag_count: 5,
        remainder_id: exited_id + 1,
        remainder_weight_kg: 1_500,
        remainder_bag_count: 15,
    };
    let exited = crate::events::Exited {
        receipt_id: exited_id,
        warehouse_id: f.warehouse.clone(),
        owner: f.farmer.clone(),
        custodian: f.custodian.clone(),
        weight_kg: 500,
        bag_count: 5,
        exit_weight_kg: 498,
        exit_date: 0,
    };

    assert_eq!(
        env.events().all(),
        std::vec![
            split.to_xdr(&env, &f.contract_id),
            exited.to_xdr(&env, &f.contract_id),
        ]
    );
}

// --- the split helper, driven directly ------------------------------------
//
// Each caller consumes a different part of `SplitParts`, so the helper is
// also exercised on its own rather than only through its callers.

#[test]
fn the_split_helper_returns_both_children_and_retires_the_parent() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-HELPER");
    let parent = f.client.get_receipt(&id);

    let parts = env.as_contract(&f.contract_id, || {
        crate::split::split_receipt(&env, &parent, 750).unwrap()
    });

    assert_eq!(parts.taken.weight_kg, 750);
    assert_eq!(parts.remainder.weight_kg, 1_250);
    assert_eq!(
        parts.taken.weight_kg + parts.remainder.weight_kg,
        parent.weight_kg
    );
    assert_eq!(parts.taken.parent_id, Some(id));
    assert_eq!(parts.remainder.parent_id, Some(id));

    // The helper divides; it does not decide what the pieces become. Both
    // children inherit the parent's status and the caller mutates from there.
    assert_eq!(parts.taken.status, Status::Active);
    assert_eq!(parts.remainder.status, Status::Active);
    assert_eq!(f.client.get_receipt(&id).status, Status::Split);

    // Both were persisted, not just returned.
    assert_eq!(f.client.get_receipt(&parts.taken.receipt_id).weight_kg, 750);
    assert_eq!(
        f.client.get_receipt(&parts.remainder.receipt_id).weight_kg,
        1_250
    );
}

#[test]
fn the_split_helper_rejects_quantities_that_are_not_a_split() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-HELPER2");
    let parent = f.client.get_receipt(&id);

    env.as_contract(&f.contract_id, || {
        assert_eq!(
            crate::split::split_receipt(&env, &parent, 0).err(),
            Some(Error::InvalidSplitAmount),
            "nothing to take"
        );
        assert_eq!(
            crate::split::split_receipt(&env, &parent, 2_000).err(),
            Some(Error::InvalidSplitAmount),
            "the whole receipt is not a split - the caller has a separate path"
        );
        assert_eq!(
            crate::split::split_receipt(&env, &parent, 2_001).err(),
            Some(Error::InvalidSplitAmount),
            "more than the receipt holds"
        );
    });

    // None of the rejected attempts touched the parent.
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);
}
