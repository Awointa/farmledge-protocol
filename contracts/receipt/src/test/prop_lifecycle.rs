//! The whole contract as a state machine. Random sequences of every
//! state-changing call — valid or not, in any order, against any receipt or
//! sale including retired and terminal ones — and after **every** step:
//!
//! 1. **Weight is conserved.** The receipts of each deposit sum to what was
//!    minted for it, a receipt's weight never grows, and every receipt split
//!    off names an earlier receipt of the same deposit.
//! 2. **A receipt is in exactly one state.** Its `status` agrees with its
//!    `lock`, `recovery` and exit fields, and an `Exited` or `Revoked`
//!    receipt never changes again.
//! 3. **A settlement is written once.** Once a sale's `settlement_ref` is set
//!    it never changes, and a second `record_settlement` fails.
//! 4. **Split lineage resolves to one sale.** Every completed recovery in a
//!    receipt's history names a sale that exists, whose sold receipt is that
//!    receipt or one of its ancestors, and whose buyer and lender match.
//! 5. **One lot is never two receipts.** Every receipt of a
//!    `deposit_no` descends from one minted receipt, and that number is
//!    claimed on-chain.
//! 6. **Anchors are strictly ordered and never ahead of the ledger.** An
//!    anchor is accepted exactly when its day is after the warehouse's last
//!    and not after the ledger's current day.
//!
//! And a rejected call leaves no side effects at all.
//!
//! Minting goes through anchors, and the books are adversarial: each day's
//! tree holds any lots, of any kind, at any weight, the same lot more than
//! once, and never marks a lot as minted. Mints are attempted from any
//! anchored day's leaves, stale ones included. Whether each anchor, mint and
//! pre-claim succeeds is predicted from a model and checked.
//!
//! `mock_all_auths` is in force: authorisation has its own tests. This one is
//! about what state the contract can be driven into by parties who *are*
//! allowed to act.

use std::collections::{BTreeMap, BTreeSet};
use std::vec;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

use super::{next_id, setup_lending};
use crate::{
    Leaf, MaybeLock, MaybeRecovery, Receipt, RecoveryResolution, Sale, Status, WeightSource,
    KIND_BALANCE,
};

/// Lots the adversarial books draw from: deposit numbers `LOT_BASE..+LOTS`.
const LOTS: u64 = 6;
const LOT_BASE: u64 = 100;

/// Which receipt an operation acts on. Most of the time it is one whose
/// status the operation accepts, so sequences actually reach locks,
/// recoveries, sales and settlements; one time in five it is any receipt at
/// all, so rejections are exercised too. (A uniformly random target almost
/// never reaches a completed sale — the earlier version of this test
/// completed one recovery in 64 cases.)
#[derive(Clone, Copy, Debug)]
struct Target {
    index: usize,
    wild: bool,
}

fn target() -> impl Strategy<Value = Target> {
    (any::<usize>(), prop::bool::weighted(0.2)).prop_map(|(index, wild)| Target { index, wild })
}

#[derive(Clone, Debug)]
enum Op {
    /// Anchor a day `step` after the last one (0 or less: out of order), or
    /// `ahead` days after the ledger's, over these (lot, kind, kg) leaves.
    Anchor {
        step: i64,
        ahead: Option<u32>,
        leaves: std::vec::Vec<(u64, u32, u64)>,
    },
    /// Mint from any leaf of the latest anchored day, or of any day.
    Mint {
        day: Option<usize>,
        leaf: usize,
    },
    Preclaim {
        lot: u64,
    },
    Lock {
        target: Target,
        pct: u64,
        lender: usize,
    },
    Unlock {
        target: Target,
        pct: u64,
    },
    Exit {
        target: Target,
        pct: u64,
    },
    Revoke {
        target: Target,
    },
    ExtendDue {
        target: Target,
        by: u64,
    },
    StartRecovery {
        target: Target,
    },
    CompleteRecovery {
        target: Target,
        buyer: usize,
    },
    Settle {
        sale: usize,
    },
    Wait {
        secs: u64,
    },
}

/// Mostly balance leaves, so mints get past the kind check often.
fn kind() -> impl Strategy<Value = u32> {
    prop_oneof![3 => Just(KIND_BALANCE), 1 => Just(0u32), 1 => Just(1u32)]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (
            -1i64..=3,
            prop::option::weighted(0.1, 0u32..3),
            prop::collection::vec((0..LOTS, kind(), 1u64..50_000), 1..5),
        )
            .prop_map(|(step, ahead, leaves)| Op::Anchor { step, ahead, leaves }),
        5 => (prop::option::weighted(0.4, any::<usize>()), any::<usize>())
            .prop_map(|(day, leaf)| Op::Mint { day, leaf }),
        1 => (0..LOTS).prop_map(|lot| Op::Preclaim { lot }),
        3 => (target(), 1u64..=100, 0usize..2)
            .prop_map(|(target, pct, lender)| Op::Lock { target, pct, lender }),
        2 => (target(), 1u64..=100).prop_map(|(target, pct)| Op::Unlock { target, pct }),
        2 => (target(), 1u64..=100).prop_map(|(target, pct)| Op::Exit { target, pct }),
        1 => target().prop_map(|target| Op::Revoke { target }),
        1 => (target(), 0u64..500).prop_map(|(target, by)| Op::ExtendDue { target, by }),
        2 => target().prop_map(|target| Op::StartRecovery { target }),
        2 => (target(), 0usize..2)
            .prop_map(|(target, buyer)| Op::CompleteRecovery { target, buyer }),
        2 => any::<usize>().prop_map(|sale| Op::Settle { sale }),
        2 => (0u64..3 * 86_400).prop_map(|secs| Op::Wait { secs }),
    ]
}

/// Every stored receipt, by id.
fn receipts(l: &super::Lending) -> BTreeMap<u64, Receipt> {
    (1..next_id(&l.f))
        .map(|id| (id, l.f.client.get_receipt(&id)))
        .collect()
}

/// Every stored sale, by id.
fn sales(l: &super::Lending) -> BTreeMap<u64, Sale> {
    let mut out = BTreeMap::new();
    let mut id = 1;
    while let Ok(Ok(sale)) = l.f.client.try_get_sale(&id) {
        out.insert(id, sale);
        id += 1;
    }
    out
}

fn is_terminal(status: &Status) -> bool {
    matches!(status, Status::Exited | Status::Revoked)
}

/// Invariant 2, for one receipt on its own.
fn check_single_state(r: &Receipt) -> Result<(), TestCaseError> {
    let id = r.receipt_id;
    let locked = matches!(r.lock, MaybeLock::Some(_));
    let recovering = matches!(r.recovery, MaybeRecovery::Some(_));
    let exited = r.exit_date.is_some() || r.exit_weight_kg.is_some();
    match r.status {
        Status::Active | Status::Revoked => {
            prop_assert!(
                !locked && !recovering && !exited,
                "{:?} {} carries state",
                r.status,
                id
            );
        }
        Status::Locked => {
            prop_assert!(
                locked && !recovering && !exited,
                "Locked {} inconsistent",
                id
            );
        }
        Status::Recovery => {
            prop_assert!(
                locked && recovering && !exited,
                "Recovery {} inconsistent",
                id
            );
            if let MaybeRecovery::Some(open) = &r.recovery {
                prop_assert_eq!(&open.resolution, &RecoveryResolution::Open);
            }
        }
        Status::Exited => {
            prop_assert!(
                !locked && !recovering && r.exit_date.is_some() && r.exit_weight_kg.is_some(),
                "Exited {} inconsistent",
                id
            );
        }
    }
    for past in r.recovery_history.iter() {
        prop_assert!(
            past.resolution != RecoveryResolution::Open,
            "an open recovery in {}'s history",
            id
        );
    }
    Ok(())
}

/// `ancestor` is `id` itself or reachable from it through `parent_id`.
fn descends_from(all: &BTreeMap<u64, Receipt>, mut id: u64, ancestor: u64) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        match all.get(&id).and_then(|r| r.parent_id) {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}

/// `id` is `ancestor`, or was split off it (directly or through other
/// receipts) by splits that all allocated ids at or after `since`.
fn split_off_since(all: &BTreeMap<u64, Receipt>, mut id: u64, ancestor: u64, since: u64) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        if id < since {
            return false;
        }
        match all.get(&id).and_then(|r| r.parent_id) {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}

fn root_of(all: &BTreeMap<u64, Receipt>, mut id: u64) -> u64 {
    while let Some(parent) = all[&id].parent_id {
        id = parent;
    }
    id
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn every_sequence_of_calls_preserves_the_invariants(
        first_kg in 2u64..1_000_000,
        bags in 0u32..1_000,
        ops in prop::collection::vec(op(), 1..60),
    ) {
        let env = Env::default();
        let l = setup_lending(&env);
        let lenders = [l.lender.clone(), Address::generate(&env)];
        let buyers = [Address::generate(&env), Address::generate(&env)];
        let wh = l.f.warehouse.clone();

        // root receipt id -> kg minted for it
        let mut minted: BTreeMap<u64, u64> = BTreeMap::new();
        minted.insert(l.f.mint_weighing(&env, "DEP-0", first_kg, bags), first_kg);
        // The model: numbers claimed, the last anchored day, and every
        // anchored day's leaves with their proofs.
        let mut claimed: BTreeSet<u64> = BTreeSet::new();
        claimed.insert(1);
        let mut latest = l.f.client.get_anchor(&wh).day;
        let mut days: std::vec::Vec<(std::vec::Vec<Leaf>, std::vec::Vec<Vec<BytesN<32>>>)> =
            std::vec::Vec::new();
        let mut salt = 0u8;

        // State that must never change once reached.
        let mut settled: BTreeMap<u64, Option<String>> = BTreeMap::new();
        // sale id -> first receipt id allocated after the sale, so the
        // receipts split off the sold one *before* the sale (a partial
        // unlock during recovery, say) are not counted as sold.
        let mut sold_after: BTreeMap<u64, u64> = BTreeMap::new();

        for op in ops {
            let head_before = l.f.client.get_anchor(&wh);
            let before = (receipts(&l), sales(&l));
            let ids: std::vec::Vec<u64> = before.0.keys().copied().collect();
            let sale_ids: std::vec::Vec<u64> = before.1.keys().copied().collect();
            let pick_from = |t: Target, accepted: &[Status]| {
                let fitting: std::vec::Vec<u64> = before
                    .0
                    .values()
                    .filter(|r| accepted.contains(&r.status))
                    .map(|r| r.receipt_id)
                    .collect();
                if t.wild || fitting.is_empty() {
                    ids[t.index % ids.len()]
                } else {
                    fitting[t.index % fitting.len()]
                }
            };
            let kg_of = |id: u64, pct: u64| (before.0[&id].weight_kg * pct / 100).max(1);
            let now = env.ledger().timestamp();
            let text = |s: &str| String::from_str(&env, s);

            let failed = match &op {
                Op::Anchor { step, ahead, leaves } => {
                    let today = (now / 86_400) as u32;
                    let day = match ahead {
                        Some(a) => today + 1 + a,
                        None => (latest as i64 + step) as u32,
                    };
                    let mut books: std::vec::Vec<Leaf> = leaves
                        .iter()
                        .map(|(lot, kind, kg)| {
                            let mut leaf = l.f.balance_leaf(&env, "DEP", *kg, 10, WeightSource::Scale);
                            leaf.deposit_no = LOT_BASE + lot;
                            leaf.kind = *kind;
                            salt = salt.wrapping_add(1);
                            leaf.salt = BytesN::from_array(&env, &[salt; 32]);
                            leaf
                        })
                        .collect();
                    for (i, leaf) in books.iter_mut().enumerate() {
                        leaf.day = day;
                        leaf.index = i as u32;
                    }
                    let tree = super::Tree::new(&env, &l.f.client, &books);
                    let res = l.f.client.try_anchor(
                        &l.f.custodian, &wh, &day, &tree.root(), &(books.len() as u32));
                    // Invariant 6.
                    let expected = day > latest && day <= today;
                    prop_assert_eq!(res.is_ok(), expected,
                        "anchor of day {} (last {}, today {})", day, latest, today);
                    if res.is_ok() {
                        latest = day;
                        let proofs = (0..books.len()).map(|i| tree.proof(&env, i)).collect();
                        days.push((books, proofs));
                    }
                    res.is_err()
                }
                Op::Mint { day, leaf } => {
                    if days.is_empty() {
                        true
                    } else {
                        let pick = day.map_or(days.len() - 1, |d| d % days.len());
                        let (books, proofs) = &days[pick];
                        let i = leaf % books.len();
                        let leaf = &books[i];
                        let res = l.f.client.try_mint_from_anchor(
                            &l.f.custodian, &l.f.verifier, leaf, &proofs[i]);
                        let expected = leaf.day == latest
                            && leaf.kind == KIND_BALANCE
                            && !claimed.contains(&leaf.deposit_no);
                        prop_assert_eq!(res.is_ok(), expected,
                            "mint of lot {} kind {} from day {} (latest {})",
                            leaf.deposit_no, leaf.kind, leaf.day, latest);
                        if let Ok(Ok(id)) = res {
                            claimed.insert(leaf.deposit_no);
                            minted.insert(id, leaf.kg);
                        }
                        res.is_err()
                    }
                }
                Op::Preclaim { lot } => {
                    let no = LOT_BASE + lot;
                    let res = l.f.client.try_preclaim(&wh, &Vec::from_array(&env, [no]));
                    prop_assert_eq!(res.is_ok(), claimed.insert(no), "preclaim of {}", no);
                    res.is_err()
                }
                Op::Lock { target, pct, lender } => {
                    let id = pick_from(*target, &[Status::Active]);
                    l.f.client
                        .try_lock(&id, &lenders[*lender], &kg_of(id, *pct), &text("LOAN"),
                                  &(now + 1_000), &100)
                        .is_err()
                }
                Op::Unlock { target, pct } => {
                    let id = pick_from(*target, &[Status::Locked, Status::Recovery]);
                    l.f.client.try_unlock(&id, &kg_of(id, *pct)).is_err()
                }
                Op::Exit { target, pct } => {
                    let id = pick_from(*target, &[Status::Active]);
                    let kg = kg_of(id, *pct);
                    l.f.client.try_exit(&id, &l.f.custodian, &kg, &kg).is_err()
                }
                Op::Revoke { target } => {
                    l.f.client
                        .try_revoke(&pick_from(*target, &[Status::Active]), &text("fraud"))
                        .is_err()
                }
                Op::ExtendDue { target, by } => {
                    let id = pick_from(*target, &[Status::Locked]);
                    let due = match &before.0[&id].lock {
                        MaybeLock::Some(lock) => lock.due_date,
                        MaybeLock::None => now,
                    };
                    l.f.client.try_extend_due(&id, &(due + by)).is_err()
                }
                Op::StartRecovery { target } => {
                    l.f.client
                        .try_start_recovery(&pick_from(*target, &[Status::Locked]))
                        .is_err()
                }
                Op::CompleteRecovery { target, buyer } => {
                    let res = l.f.client.try_complete_recovery(
                        &pick_from(*target, &[Status::Recovery]), &l.operator, &buyers[*buyer],
                        &text("SALE"));
                    if let Ok(Ok(sale_id)) = res {
                        sold_after.insert(sale_id, next_id(&l.f));
                    }
                    res.is_err()
                }
                Op::Settle { sale } => {
                    if sale_ids.is_empty() {
                        true
                    } else {
                        let sale_id = sale_ids[sale % sale_ids.len()];
                        let first_time = before.1[&sale_id].settlement_ref.is_none();
                        let res = l.f.client.try_record_settlement(
                            &sale_id, &l.operator, &text(&std::format!("PAY-{now}")));
                        // Invariant 3: the second write is always refused.
                        prop_assert_eq!(res.is_ok(), first_time,
                            "record_settlement on sale {} (already settled: {})",
                            sale_id, !first_time);
                        res.is_err()
                    }
                }
                Op::Wait { secs } => {
                    env.ledger().set_timestamp(now + secs);
                    false
                }
            };

            let after = (receipts(&l), sales(&l));
            let head_after = l.f.client.get_anchor(&wh);
            if failed {
                prop_assert_eq!(&after, &before, "a rejected {:?} left side effects", op);
                prop_assert_eq!(&head_after, &head_before, "a rejected {:?} moved the head", op);
            }
            // 6. The head only moves forward, and never past today.
            prop_assert!(head_after.day >= head_before.day);
            prop_assert!(head_after.day <= (env.ledger().timestamp() / 86_400) as u32);
            for no in &claimed {
                prop_assert!(l.f.client.is_claimed(&wh, no), "{} not claimed on-chain", no);
            }
            let (all, all_sales) = after;

            // 1. Weight conserved, per deposit; weights only shrink; lineage
            //    stays within a deposit.
            let mut total: BTreeMap<u64, u64> = BTreeMap::new();
            for r in all.values() {
                prop_assert!(r.weight_kg > 0, "receipt {} has no weight", r.receipt_id);
                *total.entry(root_of(&all, r.receipt_id)).or_default() += r.weight_kg;
                if let Some(parent_id) = r.parent_id {
                    prop_assert!(parent_id < r.receipt_id, "{} predates its parent", r.receipt_id);
                    prop_assert_eq!(&all[&parent_id].deposit_id, &r.deposit_id);
                }
                if let Some(prev) = before.0.get(&r.receipt_id) {
                    prop_assert!(r.weight_kg <= prev.weight_kg,
                        "{} grew after {:?}", r.receipt_id, op);
                }
            }
            prop_assert_eq!(&total, &minted, "weight changed after {:?}", op);

            // 5. One lot, one minted receipt: every receipt of a deposit
            //    number shares one root, and that number is claimed.
            let mut root_of_lot: BTreeMap<u64, u64> = BTreeMap::new();
            for r in all.values() {
                let root = root_of(&all, r.receipt_id);
                let first = *root_of_lot.entry(r.deposit_no).or_insert(root);
                prop_assert_eq!(first, root,
                    "deposit_no {} became two receipts, {} and {}", r.deposit_no, first, root);
                prop_assert!(claimed.contains(&r.deposit_no));
            }

            // 2. Exactly one state, and terminal receipts are frozen.
            for r in all.values() {
                check_single_state(r)?;
                if let Some(prev) = before.0.get(&r.receipt_id) {
                    if is_terminal(&prev.status) {
                        prop_assert_eq!(prev, r,
                            "terminal {} changed after {:?}", r.receipt_id, op);
                    }
                }
            }

            // 3. Settlements are write-once.
            for (sale_id, sale) in &all_sales {
                match settled.get(sale_id) {
                    Some(Some(first)) => prop_assert_eq!(
                        sale.settlement_ref.as_ref(), Some(first),
                        "settlement of sale {} changed", sale_id),
                    _ => {
                        settled.insert(*sale_id, sale.settlement_ref.clone());
                    }
                }
                prop_assert_eq!(sale.settlement_ref.is_some(), sale.settled_at.is_some());
            }

            // 4. Every completed recovery resolves to one sale on its lineage.
            for r in all.values() {
                for past in r.recovery_history.iter() {
                    if past.resolution != RecoveryResolution::Completed {
                        prop_assert!(past.sale_id.is_none());
                        continue;
                    }
                    let sale_id = past.sale_id.expect("a completed recovery names its sale");
                    let sale = all_sales.get(&sale_id);
                    prop_assert!(sale.is_some(), "{} names missing sale {}", r.receipt_id, sale_id);
                    let sale = sale.unwrap();
                    prop_assert!(descends_from(&all, r.receipt_id, sale.receipt_id),
                        "{} names sale {} of unrelated receipt {}",
                        r.receipt_id, sale_id, sale.receipt_id);
                    prop_assert_eq!(Some(&sale.buyer), past.buyer.as_ref());
                    prop_assert_eq!(&sale.lender, &past.lender);
                    prop_assert!(r.weight_kg <= sale.weight_kg);
                }
            }
            // ...and the sold receipt plus everything split off it since the
            // sale account for exactly what was sold, so a sale's grain is
            // never claimed twice.
            for (sale_id, sale) in &all_sales {
                let watermark = sold_after[sale_id];
                let sold: u64 = all
                    .values()
                    .filter(|r| split_off_since(&all, r.receipt_id, sale.receipt_id, watermark))
                    .map(|r| r.weight_kg)
                    .sum();
                prop_assert_eq!(sold, sale.weight_kg, "sale {} over- or under-claimed", sale_id);
            }
        }
    }
}
