//! The one place a receipt is ever divided.
//!
//! Splitting is **internal to an operation**, never a callable entry point.
//! `exit()` uses it today; `lock()` and `transfer()` reuse it next session.
//!
//! The model is a paper receipt being voided and reissued as two: the parent
//! moves to [`Status::Split`] and is never mutated again, and exactly two
//! children are created whose weights sum to it. Repeated splits therefore
//! form an immutable binary tree in which the leaf weights always sum to the
//! root — the invariant `prop_split.rs` fuzzes.

use soroban_sdk::Env;

use crate::errors::Error;
use crate::events;
use crate::storage;
use crate::types::{Receipt, Status};

/// Both children of a split.
pub struct SplitParts {
    /// The `kg` the operation asked for. The caller mutates this one —
    /// `exit` retires it, `lock` will pledge it.
    pub taken: Receipt,
    /// What is left. Keeps the parent's status, so after a partial exit the
    /// remainder is still `Active`.
    ///
    /// `exit` has no use for it beyond having it persisted; `unlock` and
    /// `transfer` will read it next session.
    #[cfg_attr(not(test), allow(dead_code))]
    pub remainder: Receipt,
}

/// Divides `parent` into `kg` and `parent.weight_kg - kg`.
///
/// Writes all three receipts and indexes both children. Returns them for the
/// caller to finish mutating and re-persist.
///
/// The caller is responsible for checking `parent.status` — `exit` and (next
/// session) `lock` accept different sets. This function only enforces that
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
    // over so the two children's bag counts also sum exactly to the parent's.
    // A very small `kg` can round to zero bags; that is correct for a
    // part-bag withdrawal, and `weight_kg` remains the authoritative quantity.
    let taken_bags = ((parent.bag_count as u128 * kg as u128) / parent.weight_kg as u128) as u32;
    let remainder_bags = parent.bag_count - taken_bags;

    let taken = child_of(env, parent, taken_weight, taken_bags);
    let remainder = child_of(env, parent, remainder_weight, remainder_bags);

    // Retire the parent. This is its last write.
    let mut retired = parent.clone();
    retired.status = Status::Split;
    storage::set_receipt(env, &retired);

    storage::set_receipt(env, &taken);
    storage::set_receipt(env, &remainder);

    for child in [&taken, &remainder] {
        storage::index_owner(env, &child.owner, child.receipt_id);
        storage::index_warehouse(env, &child.warehouse_id, child.receipt_id);
    }

    events::split(
        env,
        parent.receipt_id,
        taken.receipt_id,
        taken_weight,
        remainder.receipt_id,
        remainder_weight,
    );

    Ok(SplitParts { taken, remainder })
}

/// A child inherits everything identifying about the deposit — including
/// `deposit_id`, since both halves are the same physical intake — and carries
/// the parent's status and lock/recovery state forward. Only identity,
/// quantity and lineage change.
fn child_of(env: &Env, parent: &Receipt, weight_kg: u64, bag_count: u32) -> Receipt {
    Receipt {
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
        deposit_date: parent.deposit_date,
        exit_date: None,
        exit_weight_kg: None,
    }
}
