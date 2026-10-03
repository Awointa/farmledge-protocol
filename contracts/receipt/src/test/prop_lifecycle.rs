//! The whole contract as a state machine. Random sequences of every
//! state-changing call — valid or not, in any order, against any receipt or
//! sale including retired and terminal ones — and after **every** step:
//!
//! 1. **Weight is conserved.** The non-`Split` receipts of each deposit sum to
//!    what was minted for it, and every split parent is covered exactly by
//!    its two children.
//! 2. **A receipt is in exactly one state.** Its `status` agrees with its
//!    `lock`, `recovery` and exit fields; a `Split` parent is frozen at the
//!    moment it was split; nothing ever leaves a terminal status.
//! 3. **A settlement is written once.** Once a sale's `settlement_ref` is set
//!    it never changes, and a second `record_settlement` fails.
//! 4. **Split lineage resolves to one sale.** Every completed recovery in a
//!    receipt's history names a sale that exists, whose sold receipt is that
//!    receipt or one of its ancestors, and whose buyer and lender match.
//!
//! And a rejected call leaves no side effects at all.
//!
//! `mock_all_auths` is in force: authorisation has its own tests. This one is
//! about what state the contract can be driven into by parties who *are*
//! allowed to act.

use std::collections::BTreeMap;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

use super::{next_id, prop_split::assert_each_parent_is_covered_by_its_children, setup_lending};
use crate::{MaybeLock, MaybeRecovery, Receipt, RecoveryResolution, Sale, Status};

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
    Mint {
        kg: u64,
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

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        1 => (1u64..50_000).prop_map(|kg| Op::Mint { kg }),
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
        2 => (0u64..3_000).prop_map(|secs| Op::Wait { secs }),
    ]
}

/// Every stored receipt, split parents included, by id.
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
    matches!(status, Status::Split | Status::Exited | Status::Revoked)
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
        // A split parent keeps whatever it had when it was divided; its
        // children carry that state forward. Frozen-ness is checked across
        // steps instead.
        Status::Split => prop_assert!(!exited, "Split {} has exit data", id),
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
        ops in prop::collection::vec(op(), 1..40),
    ) {
        let env = Env::default();
        let l = setup_lending(&env);
        let lenders = [l.lender.clone(), Address::generate(&env)];
        let buyers = [Address::generate(&env), Address::generate(&env)];

        // root receipt id -> kg minted for it
        let mut minted: BTreeMap<u64, u64> = BTreeMap::new();
        minted.insert(l.f.mint_weighing(&env, "DEP-0", first_kg, bags), first_kg);
        let mut deposits = 1u32;

        // State that must never change once reached.
        let mut frozen: BTreeMap<u64, Receipt> = BTreeMap::new();
        let mut settled: BTreeMap<u64, Option<String>> = BTreeMap::new();

        for op in ops {
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
                Op::Mint { kg } => {
                    let deposit = std::format!("DEP-{deposits}");
                    deposits += 1;
                    let id = l.f.mint_weighing(&env, &deposit, *kg, 10);
                    minted.insert(id, *kg);
                    false
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
                    l.f.client
                        .try_complete_recovery(&pick_from(*target, &[Status::Recovery]), &l.operator, &buyers[*buyer],
                                               &text("SALE"))
                        .is_err()
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
            if failed {
                prop_assert_eq!(&after, &before, "a rejected {:?} left side effects", op);
            }
            let (all, all_sales) = after;

            // 1. Weight conserved, per deposit and per split.
            let mut live: BTreeMap<u64, u64> = BTreeMap::new();
            for r in all.values().filter(|r| r.status != Status::Split) {
                prop_assert!(r.weight_kg > 0, "receipt {} has no weight", r.receipt_id);
                *live.entry(root_of(&all, r.receipt_id)).or_default() += r.weight_kg;
            }
            prop_assert_eq!(&live, &minted, "weight changed after {:?}", op);
            assert_each_parent_is_covered_by_its_children(&l.f, next_id(&l.f) - 1)?;

            // 2. Exactly one state, and terminal states are final.
            for r in all.values() {
                check_single_state(r)?;
                if let Some(prev) = before.0.get(&r.receipt_id) {
                    if is_terminal(&prev.status) {
                        prop_assert_eq!(&prev.status, &r.status,
                            "{} left terminal status after {:?}", r.receipt_id, op);
                    }
                }
                if r.status == Status::Split {
                    let snapshot = frozen.entry(r.receipt_id).or_insert_with(|| r.clone());
                    prop_assert_eq!(&*snapshot, r, "split parent {} changed", r.receipt_id);
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
            // ...and a sold receipt's descendants account for exactly what
            // was sold, so a sale's grain is never claimed twice.
            for sale in all_sales.values() {
                let sold_leaves: u64 = all
                    .values()
                    .filter(|r| r.status != Status::Split)
                    .filter(|r| descends_from(&all, r.receipt_id, sale.receipt_id))
                    .map(|r| r.weight_kg)
                    .sum();
                prop_assert_eq!(sold_leaves, sale.weight_kg);
            }
        }
    }
}
