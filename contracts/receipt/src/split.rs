//! The one place a receipt is ever divided.
//!
//! Splitting is **internal to an operation**, never a callable entry point.
//! `exit`, `lock` and `unlock` all divide receipts through this one helper.
//!
//! A split happens **in place**: the receipt keeps its id, status, lock and
//! recovery, and only its `weight_kg` and `bag_count` drop. The portion the
//! operation acts on becomes one new receipt whose `parent_id` names it.
//! Writing one receipt instead of retiring the parent and writing two halves
//! the rent of every partial operation (measured in `docs/archival.md`).
//!
//! The trade: a receipt's weight is not fixed for life. Its history is the
//! chain of `Split` events, each carrying the weight before and both
//! weights after, plus `parent_id` on every receipt split off. The
//! invariant `prop_split.rs` fuzzes is that the receipts of one deposit
//! always sum to its minted weight and bag count.

use soroban_sdk::Env;

use crate::errors::Error;
use crate::events;
use crate::storage;
use crate::types::{MaybeLock, Receipt, DATA_VERSION};

/// Both sides of a split.
pub struct SplitParts {
    /// The new receipt holding the `kg` the operation asked for. The caller
    /// mutates this one: `exit` retires it, `lock` pledges it, `unlock`
    /// releases it.
    pub taken: Receipt,
    /// The original receipt, under its own id, holding what is left. It
    /// keeps its status, lock and recovery, so after a partial exit or lock
    /// it is still `Active`, and after a partial unlock it is still
    /// `Locked` or in `Recovery`.
    pub remainder: Receipt,
}

/// Divides `parent` into `kg` (a new receipt) and `parent.weight_kg - kg`
/// (the parent itself).
///
/// Writes both receipts and indexes the new one. Returns them for the
/// caller to finish mutating and re-persist.
///
/// The caller is responsible for checking `parent.status` — `exit`, `lock`
/// and `unlock` accept different sets. This function only enforces that
/// the quantity actually describes a split: `0 < kg < parent.weight_kg`. An
/// exact-weight "split" is [`Error::InvalidSplitAmount`], because the caller
/// should have taken its whole-receipt path instead.
pub fn split_receipt(env: &Env, parent: &Receipt, kg: u64) -> Result<SplitParts, Error> {
    if kg == 0 || kg >= parent.weight_kg {
        return Err(Error::InvalidSplitAmount);
    }

    let taken_weight = kg;
    let remainder_weight = parent.weight_kg - kg;

    // Pro-rate bags by weight, giving the remainder whatever rounding is left
    // over so the two sides' bag counts also sum exactly to the parent's.
    // A very small `kg` can round to zero bags; that is correct for a
    // part-bag withdrawal, and `weight_kg` remains the authoritative quantity.
    let taken_bags = ((parent.bag_count as u128 * kg as u128) / parent.weight_kg as u128) as u32;
    let remainder_bags = parent.bag_count - taken_bags;

    let taken = child_of(env, parent, taken_weight, taken_bags);
    let mut remainder = parent.clone();
    remainder.weight_kg = remainder_weight;
    remainder.bag_count = remainder_bags;

    storage::set_receipt(env, &remainder);
    storage::set_receipt(env, &taken);

    // The parent is already in every index it belongs to.
    storage::index_owner(env, &taken.owner, taken.receipt_id);
    storage::index_warehouse(env, &taken.warehouse_id, taken.receipt_id);
    // Split off a pledged receipt, the new one starts out pledged too, so
    // the lender's index must follow it.
    if let MaybeLock::Some(lock) = &taken.lock {
        storage::index_lender(env, &lock.lender, taken.receipt_id);
    }

    events::Split {
        parent_id: parent.receipt_id,
        owner: parent.owner.clone(),
        warehouse_id: parent.warehouse_id.clone(),
        status: parent.status.clone(),
        lender: match &parent.lock {
            MaybeLock::Some(lock) => Some(lock.lender.clone()),
            MaybeLock::None => None,
        },
        parent_weight_kg: parent.weight_kg,
        taken_id: taken.receipt_id,
        taken_weight_kg: taken_weight,
        taken_bag_count: taken_bags,
        remainder_id: remainder.receipt_id,
        remainder_weight_kg: remainder_weight,
        remainder_bag_count: remainder_bags,
        split_at: env.ledger().timestamp(),
    }
    .publish(env);

    Ok(SplitParts { taken, remainder })
}

/// The new receipt inherits everything identifying about the deposit —
/// including `deposit_id`, since it is the same physical intake — and
/// carries the parent's status and lock/recovery state forward. Only
/// identity, quantity and lineage differ.
fn child_of(env: &Env, parent: &Receipt, weight_kg: u64, bag_count: u32) -> Receipt {
    Receipt {
        data_version: DATA_VERSION,
        receipt_id: storage::next_receipt_id(env),
        deposit_id: parent.deposit_id.clone(),
        parent_id: Some(parent.receipt_id),
        commodity: parent.commodity.clone(),
        grade: parent.grade.clone(),
        weight_kg,
        weight_source: parent.weight_source.clone(),
        bag_count,
        moisture_pct: parent.moisture_pct,
        warehouse_id: parent.warehouse_id.clone(),
        custodian: parent.custodian.clone(),
        verifier: parent.verifier.clone(),
        owner: parent.owner.clone(),
        status: parent.status.clone(),
        lock: parent.lock.clone(),
        recovery: parent.recovery.clone(),
        recovery_history: parent.recovery_history.clone(),
        deposit_date: parent.deposit_date,
        exit_date: None,
        exit_weight_kg: None,
    }
}
