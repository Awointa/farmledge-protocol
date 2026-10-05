//! The invariant everything later depends on: **splitting never creates or
//! destroys grain.**
//!
//! `exit`, `lock` and `unlock` all divide receipts through
//! `split::split_receipt`, so this property is the contract's safety net
//! rather than a test of `exit` specifically. It drives the helper through
//! `exit`; `prop_lifecycle.rs` drives it through every caller.
//!
//! A split reduces the parent in place and adds one receipt naming it as
//! `parent_id`, so the receipts of a deposit form a tree rooted at the
//! minted receipt, and every node's current weight plus everything split
//! off below it is what that node held when it was created.

use proptest::prelude::*;
use soroban_sdk::Env;

use super::{receipts, setup, total_bags, total_weight, Fixture};
use crate::Status;

/// Weight and bags of `id` plus everything split off it, recursively.
fn subtree(all: &[crate::Receipt], id: u64) -> (u64, u32) {
    let me = all.iter().find(|r| r.receipt_id == id).unwrap();
    all.iter()
        .filter(|r| r.parent_id == Some(id))
        .map(|child| subtree(all, child.receipt_id))
        .fold((me.weight_kg, me.bag_count), |(w, b), (cw, cb)| {
            (w + cw, b + cb)
        })
}

/// The tree rooted at `root` still accounts for the whole deposit, and
/// every split-off receipt names an earlier receipt of the same deposit.
pub fn assert_lineage_accounts_for_the_deposit(
    f: &Fixture,
    root: u64,
    highest_id: u64,
    minted_kg: u64,
    minted_bags: u32,
) -> Result<(), TestCaseError> {
    let all = receipts(f, highest_id);
    let root_receipt = all.iter().find(|r| r.receipt_id == root).unwrap();
    for r in &all {
        if let Some(parent_id) = r.parent_id {
            prop_assert!(parent_id < r.receipt_id, "a parent predates its child");
            let parent = all.iter().find(|p| p.receipt_id == parent_id).unwrap();
            prop_assert_eq!(&parent.deposit_id, &r.deposit_id, "same physical deposit");
        } else {
            prop_assert_eq!(r.receipt_id, root, "only the minted receipt has no parent");
        }
        prop_assert_eq!(&r.deposit_id, &root_receipt.deposit_id);
    }
    prop_assert_eq!(subtree(&all, root), (minted_kg, minted_bags));
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
        let mut highest = root;

        for pct in cuts {
            // The root keeps whatever is left after each split.
            let current = f.client.get_receipt(&root);
            let kg = current.weight_kg * pct as u64 / 100;
            if kg == 0 || kg >= current.weight_kg {
                continue; // not a split; exit() would take its whole-receipt path
            }
            highest = f.client.exit(&root, &f.custodian, &kg, &kg);
        }

        prop_assert_eq!(total_weight(&f, highest), total_kg, "weight is conserved");
        prop_assert_eq!(total_bags(&f, highest), bags, "bag count is conserved");
        prop_assert_eq!(f.client.get_receipt(&root).status, Status::Active);
        assert_lineage_accounts_for_the_deposit(&f, root, highest, total_kg, bags)?;
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

        let taken = f.client.get_receipt(&taken);
        let remainder = f.client.get_receipt(&root);

        prop_assert_eq!(taken.weight_kg + remainder.weight_kg, total_kg);
        prop_assert_eq!(taken.bag_count + remainder.bag_count, bags);
        prop_assert_eq!(taken.weight_kg, kg);
        prop_assert_eq!(taken.status, Status::Exited);
        prop_assert_eq!(taken.parent_id, Some(root));
        prop_assert_eq!(remainder.status, Status::Active);
        prop_assert_eq!(remainder.parent_id, None);
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
        let mut highest = root;

        for pct in cuts {
            let current = f.client.get_receipt(&root);
            let kg = current.weight_kg * pct as u64 / 100;
            if kg == 0 || kg >= current.weight_kg {
                continue;
            }
            highest = f.client.exit(&root, &f.custodian, &kg, &kg);
        }

        for receipt in receipts(&f, highest) {
            prop_assert!(receipt.weight_kg > 0, "receipt {} has no weight", receipt.receipt_id);
        }
    }
}
