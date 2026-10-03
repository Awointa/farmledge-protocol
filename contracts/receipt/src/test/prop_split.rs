//! The invariant everything later depends on: **splitting never creates or
//! destroys grain.**
//!
//! `exit`, `lock` and `unlock` all divide receipts through
//! `split::split_receipt`, so this property is the contract's safety net
//! rather than a test of `exit` specifically. It drives the helper through
//! `exit`; `prop_lifecycle.rs` drives it through every caller.

use proptest::prelude::*;
use soroban_sdk::Env;

use super::{leaves, live_bags, live_weight, setup, Fixture};
use crate::Status;

/// For every receipt that was split, check its two children account for it
/// exactly — in weight and in bags.
pub fn assert_each_parent_is_covered_by_its_children(
    f: &Fixture,
    highest_id: u64,
) -> Result<(), TestCaseError> {
    let mut children: std::collections::BTreeMap<u64, (u32, u64, u32)> = Default::default();

    for id in 1..=highest_id {
        if let Ok(Ok(r)) = f.client.try_get_receipt(&id) {
            if let Some(parent_id) = r.parent_id {
                let entry = children.entry(parent_id).or_insert((0, 0, 0));
                entry.0 += 1;
                entry.1 += r.weight_kg;
                entry.2 += r.bag_count;
            }
        }
    }

    for (parent_id, (count, weight, bags)) in children {
        let parent = f.client.get_receipt(&parent_id);
        prop_assert_eq!(count, 2, "a split produces exactly two children");
        prop_assert_eq!(parent.status, Status::Split, "a split parent is retired");
        prop_assert_eq!(weight, parent.weight_kg, "children's weight sums to parent");
        prop_assert_eq!(bags, parent.bag_count, "children's bags sum to parent");
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Repeatedly shave a percentage off whatever is still active, then check
    /// that the surviving receipts still account for every kilogram and every
    /// bag of the original deposit.
    #[test]
    fn repeated_splits_conserve_weight_and_bags(
        total_kg in 1_000u64..5_000_000,
        bags in 0u32..2_000,
        cuts in prop::collection::vec(1u32..=99u32, 1..10),
    ) {
        let env = Env::default();
        let f = setup(&env);

        let root = f.mint_weighing(&env, "DEP-PROP", total_kg, bags);
        let mut active = root;
        let mut highest = root;

        for pct in cuts {
            let current = f.client.get_receipt(&active);
            let kg = current.weight_kg * pct as u64 / 100;
            if kg == 0 || kg >= current.weight_kg {
                continue; // not a split; exit() would take its whole-receipt path
            }
            // The split allocates `taken` then `remainder`, so the remainder
            // is always the next id after the exited child.
            let taken = f.client.exit(&active, &f.custodian, &kg, &kg);
            active = taken + 1;
            highest = active;
        }

        prop_assert_eq!(live_weight(&f, highest), total_kg, "weight is conserved");
        prop_assert_eq!(live_bags(&f, highest), bags, "bag count is conserved");
        assert_each_parent_is_covered_by_its_children(&f, highest)?;
    }

    /// A single split of any valid size, including the extreme ends of the
    /// range where rounding is most likely to lose a kilogram or a bag.
    #[test]
    fn one_split_conserves_at_any_ratio(
        total_kg in 2u64..10_000_000,
        bags in 0u32..5_000,
        kg in 1u64..10_000_000,
    ) {
        prop_assume!(kg < total_kg);

        let env = Env::default();
        let f = setup(&env);

        let root = f.mint_weighing(&env, "DEP-ONE", total_kg, bags);
        let taken = f.client.exit(&root, &f.custodian, &kg, &kg);
        let remainder = taken + 1;

        let taken = f.client.get_receipt(&taken);
        let remainder = f.client.get_receipt(&remainder);

        prop_assert_eq!(taken.weight_kg + remainder.weight_kg, total_kg);
        prop_assert_eq!(taken.bag_count + remainder.bag_count, bags);
        prop_assert_eq!(taken.weight_kg, kg);
        prop_assert_eq!(taken.status, Status::Exited);
        prop_assert_eq!(remainder.status, Status::Active);
        prop_assert_eq!(f.client.get_receipt(&root).status, Status::Split);
    }

    /// No split may ever mint a weightless receipt: a zero-weight receipt
    /// would be unexitable (exit rejects `kg == 0`) and would corrupt the
    /// conservation arithmetic for every later split.
    #[test]
    fn splits_never_produce_a_zero_weight_receipt(
        total_kg in 2u64..100_000,
        cuts in prop::collection::vec(1u32..=99u32, 1..6),
    ) {
        let env = Env::default();
        let f = setup(&env);

        let root = f.mint_weighing(&env, "DEP-ZERO", total_kg, 10);
        let mut active = root;
        let mut highest = root;

        for pct in cuts {
            let current = f.client.get_receipt(&active);
            let kg = current.weight_kg * pct as u64 / 100;
            if kg == 0 || kg >= current.weight_kg {
                continue;
            }
            let taken = f.client.exit(&active, &f.custodian, &kg, &kg);
            active = taken + 1;
            highest = active;
        }

        for receipt in leaves(&f, highest) {
            prop_assert!(receipt.weight_kg > 0, "receipt {} has no weight", receipt.receipt_id);
        }
    }
}
