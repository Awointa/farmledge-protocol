//! Focused properties, each driven straight at one invariant so it is
//! exercised hundreds of times rather than whenever the whole-contract state
//! machine (`prop_lifecycle.rs`) happens to reach it.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

use super::{setup, setup_lending};
use crate::{Error, WeightSource};

const DAY: u64 = 86_400;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Anchors are accepted exactly when the day is after the last anchored
    /// one and not after the ledger's current day. The accepted days are
    /// strictly increasing, and none was ever ahead of the ledger.
    #[test]
    fn anchors_are_strictly_ordered_and_never_ahead_of_the_ledger(
        steps in prop::collection::vec((-400i64..5, 0u64..3 * DAY), 1..40),
    ) {
        let env = Env::default();
        let f = setup(&env);
        let root = BytesN::from_array(&env, &[7; 32]);
        let mut last: Option<u32> = None;
        let mut accepted = std::vec::Vec::new();

        for (offset, wait) in steps {
            let now = env.ledger().timestamp();
            let today = (now / DAY) as u32;
            let day = (today as i64 + offset).max(0) as u32;
            let res = f.client.try_anchor(&f.custodian, &f.warehouse, &day, &root, &1);

            let expected = last.is_none_or(|l| day > l) && day <= today;
            prop_assert_eq!(res.is_ok(), expected, "day {} (last {:?}, today {})", day, last, today);
            match res {
                Ok(Ok(_)) => {
                    last = Some(day);
                    accepted.push((day, today));
                }
                Err(Ok(e)) => prop_assert!(
                    e == if day > today { Error::AnchorInFuture } else { Error::AnchorOutOfOrder },
                    "day {} refused with {:?}", day, e
                ),
                other => prop_assert!(false, "unexpected {:?}", other),
            }
            prop_assert_eq!(f.client.try_get_anchor(&f.warehouse).ok().and_then(|r| r.ok()).map(|a| a.day), last);
            env.ledger().set_timestamp(now + wait);
        }
        for pair in accepted.windows(2) {
            prop_assert!(pair[0].0 < pair[1].0);
        }
        for (day, today) in accepted {
            prop_assert!(day <= today);
        }
    }

    /// The same lots, re-anchored day after day at shifting positions, with
    /// mints attempted from every leaf of every day: each lot becomes at most
    /// one receipt, and only from the latest day.
    #[test]
    fn one_lot_never_becomes_two_receipts(
        days in prop::collection::vec(prop::collection::vec(0u64..8, 1..8), 1..6),
        order in prop::collection::vec(any::<prop::sample::Index>(), 0..40),
    ) {
        let env = Env::default();
        let f = setup(&env);
        let mut anchored = std::vec::Vec::new();
        for lots in &days {
            let mut leaves: std::vec::Vec<_> = lots
                .iter()
                .enumerate()
                .map(|(i, lot)| {
                    let mut leaf = f.balance_leaf(&env, "DEP", 1_000 + *lot, 10, WeightSource::Scale);
                    leaf.deposit_no = *lot;
                    leaf.salt = BytesN::from_array(&env, &[(anchored.len() * 16 + i) as u8; 32]);
                    leaf
                })
                .collect();
            let proofs = f.anchor_next_day(&env, &mut leaves);
            anchored.push((leaves, proofs));
        }
        let latest = anchored.len() - 1;
        let all: std::vec::Vec<(usize, usize)> = anchored
            .iter()
            .enumerate()
            .flat_map(|(d, (leaves, _))| (0..leaves.len()).map(move |i| (d, i)))
            .collect();

        let mut receipts: BTreeMap<u64, u64> = BTreeMap::new();
        let mut attempts: std::vec::Vec<(usize, usize)> = order.iter().map(|ix| *ix.get(&all)).collect();
        attempts.extend(all.iter().copied());
        for (d, i) in attempts {
            let (leaves, proofs) = &anchored[d];
            let leaf = &leaves[i];
            let res = f.client.try_mint_from_anchor(&f.custodian, &f.verifier, leaf, &proofs[i]);
            let expected = d == latest && !receipts.contains_key(&leaf.deposit_no);
            prop_assert_eq!(res.is_ok(), expected, "lot {} from day {} of {}", leaf.deposit_no, d, latest);
            if let Ok(Ok(id)) = res {
                receipts.insert(leaf.deposit_no, id);
            }
        }
        let lots_today: BTreeSet<u64> = days[latest].iter().copied().collect();
        prop_assert_eq!(receipts.keys().copied().collect::<BTreeSet<_>>(), lots_today);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any sequence of settlement attempts, against any of several sales,
    /// interleaved with the buyers splitting and exiting what they bought:
    /// each sale's first settlement is kept, and every later one is refused
    /// and changes nothing.
    #[test]
    fn a_settlement_is_never_written_twice(
        sales in 1usize..4,
        ops in prop::collection::vec((0usize..4, 0usize..3, 0u8..3), 1..30),
    ) {
        let env = Env::default();
        let l = setup_lending(&env);
        let buyer = Address::generate(&env);
        let mut sale_ids = std::vec::Vec::new();
        let mut bought = std::vec::Vec::new();
        let ids: std::vec::Vec<u64> = (0..sales).map(|i| l.f.mint(&env, &std::format!("DEP-{i}"))).collect();
        for id in &ids {
            l.lock(&env, *id, 2_000);
        }
        env.ledger().set_timestamp(super::DUE + super::GRACE + 1);
        for id in &ids {
            l.f.client.start_recovery(id);
            sale_ids.push(l.f.client.complete_recovery(id, &l.operator, &buyer, &String::from_str(&env, "SALE")));
            bought.push(*id);
        }

        let mut first: BTreeMap<u64, String> = BTreeMap::new();
        for (sale, reference, action) in ops {
            let sale_id = sale_ids[sale % sale_ids.len()];
            match action {
                0 | 1 => {
                    let r = String::from_str(&env, &std::format!("PAY-{reference}"));
                    let res = l.f.client.try_record_settlement(&sale_id, &l.operator, &r);
                    prop_assert_eq!(res.is_ok(), !first.contains_key(&sale_id));
                    if res.is_err() {
                        prop_assert_eq!(res, Err(Ok(Error::RecoveryAlreadySettled)));
                    }
                    first.entry(sale_id).or_insert(r);
                }
                _ => {
                    // The buyer takes part of the grain out, splitting the
                    // sold receipt; settlement must still be one record.
                    let id = bought[sale % bought.len()];
                    let _ = l.f.client.try_exit(&id, &l.f.custodian, &100, &100);
                }
            }
            for (sale_id, r) in &first {
                prop_assert_eq!(l.f.client.get_sale(sale_id).settlement_ref, Some(r.clone()));
            }
        }
        let unsettled: Vec<crate::Sale> = l.f.client.get_unsettled_sales(&0);
        prop_assert_eq!(unsettled.len() as usize, sale_ids.len() - first.len());
    }
}
