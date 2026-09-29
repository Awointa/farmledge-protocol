//! No sequence of lending and exit calls — valid or not, in any order — may
//! create or destroy grain, produce a weightless receipt, or leave a failed
//! call with side effects.
//!
//! Operations target an arbitrary receipt by index, including retired and
//! terminal ones, so most generated calls are *supposed* to fail. That is the
//! point: the property covers the rejection paths as much as the happy ones.

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

use super::{
    leaves, next_id, prop_split::assert_each_parent_is_covered_by_its_children, setup_lending,
};
use crate::Receipt;

#[derive(Clone, Debug)]
enum Op {
    Lock {
        target: usize,
        pct: u64,
        second_lender: bool,
    },
    Unlock {
        target: usize,
        pct: u64,
    },
    Exit {
        target: usize,
        pct: u64,
    },
    ExtendDue {
        target: usize,
        by: u64,
    },
    StartRecovery {
        target: usize,
    },
    CompleteRecovery {
        target: usize,
    },
    Wait {
        secs: u64,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (any::<usize>(), 1u64..=100, any::<bool>()).prop_map(|(target, pct, second_lender)| {
            Op::Lock {
                target,
                pct,
                second_lender,
            }
        }),
        (any::<usize>(), 1u64..=100).prop_map(|(target, pct)| Op::Unlock { target, pct }),
        (any::<usize>(), 1u64..=100).prop_map(|(target, pct)| Op::Exit { target, pct }),
        (any::<usize>(), 0u64..500).prop_map(|(target, by)| Op::ExtendDue { target, by }),
        any::<usize>().prop_map(|target| Op::StartRecovery { target }),
        any::<usize>().prop_map(|target| Op::CompleteRecovery { target }),
        (0u64..3_000).prop_map(|secs| Op::Wait { secs }),
    ]
}

/// Every stored receipt, split parents included.
fn snapshot(f: &super::Fixture) -> std::vec::Vec<Receipt> {
    (1..next_id(f))
        .map(|id| f.client.get_receipt(&id))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_sequence_of_calls_changes_total_weight(
        total_kg in 2u64..1_000_000,
        bags in 0u32..1_000,
        ops in prop::collection::vec(op(), 1..30),
    ) {
        let env = Env::default();
        let l = setup_lending(&env);
        let lender2 = Address::generate(&env);
        let buyer = Address::generate(&env);
        l.f.mint_weighing(&env, "DEP-PROP", total_kg, bags);

        for op in ops {
            let before = snapshot(&l.f);
            let ids: std::vec::Vec<u64> = (1..next_id(&l.f)).collect();
            let pick = |t: usize| ids[t % ids.len()];
            let kg_of = |id: u64, pct: u64| {
                // Always at least 1kg, and exactly the whole receipt at 100%.
                (l.f.client.get_receipt(&id).weight_kg * pct / 100).max(1)
            };
            let now = env.ledger().timestamp();

            let failed = match op {
                Op::Lock { target, pct, second_lender } => {
                    let id = pick(target);
                    let lender = if second_lender { &lender2 } else { &l.lender };
                    l.f.client
                        .try_lock(
                            &id,
                            lender,
                            &kg_of(id, pct),
                            &String::from_str(&env, "LOAN-P"),
                            &(now + 1_000),
                            &100,
                        )
                        .is_err()
                }
                Op::Unlock { target, pct } => {
                    let id = pick(target);
                    l.f.client.try_unlock(&id, &kg_of(id, pct)).is_err()
                }
                Op::Exit { target, pct } => {
                    let id = pick(target);
                    let kg = kg_of(id, pct);
                    l.f.client.try_exit(&id, &l.f.custodian, &kg, &kg).is_err()
                }
                Op::ExtendDue { target, by } => {
                    let id = pick(target);
                    let due = l.f.client.try_get_lock(&id).ok().and_then(|r| r.ok())
                        .map_or(now, |lock| lock.due_date);
                    l.f.client.try_extend_due(&id, &(due + by)).is_err()
                }
                Op::StartRecovery { target } => {
                    l.f.client.try_start_recovery(&pick(target)).is_err()
                }
                Op::CompleteRecovery { target } => {
                    l.f.client
                        .try_complete_recovery(
                            &pick(target),
                            &l.operator,
                            &buyer,
                            &String::from_str(&env, "SALE-P"),
                            &String::from_str(&env, "SETTLE-P"),
                        )
                        .is_err()
                }
                Op::Wait { secs } => {
                    env.ledger().set_timestamp(now + secs);
                    false
                }
            };

            if failed {
                prop_assert_eq!(&snapshot(&l.f), &before, "a rejected {:?} left side effects", op);
            }

            let highest = next_id(&l.f) - 1;
            let live = leaves(&l.f, highest);
            prop_assert_eq!(
                live.iter().map(|r| r.weight_kg).sum::<u64>(),
                total_kg,
                "weight changed after {:?}", op
            );
            prop_assert_eq!(live.iter().map(|r| r.bag_count).sum::<u32>(), bags);
            for r in &live {
                prop_assert!(r.weight_kg > 0, "receipt {} has no weight", r.receipt_id);
            }
            assert_each_parent_is_covered_by_its_children(&l.f, highest)?;
        }
    }
}
