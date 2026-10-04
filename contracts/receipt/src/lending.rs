//! The lending lifecycle: `lock`, `unlock`, `extend_due`, `start_recovery`,
//! `complete_recovery`, `record_settlement`.
//!
//! Only `lock` takes the lender as a parameter, because the pledge does not
//! exist yet. Every later operation authorises the lender **recorded in the
//! `Lock`**, never a caller-supplied address — the same reasoning as
//! `admin::require_admin`.
//!
//! All time comparisons use the ledger timestamp. Loan terms (rates, fees,
//! how long a grace period should be) are the lender's business and never
//! reach the chain; the contract only enforces that dates are ordered.

use soroban_sdk::{Address, Env, String};

use crate::admin;
use crate::errors::Error;
use crate::events;
use crate::split;
use crate::storage;
use crate::types::{
    Lock, MaybeLock, MaybeRecovery, Receipt, Recovery, RecoveryResolution, Sale, Status,
    WeightSource, DATA_VERSION,
};

/// Longest `deposit_id`, `loan_ref`, `sale_ref` or `settlement_ref`
/// accepted, in bytes.
///
/// The point is the bound, not the bytes saved: each ref is stored in the
/// receipt, `Lock`, every `Recovery` or the `Sale`, and copied onto every
/// receipt split off, so an unbounded ref would let a caller inflate the
/// rent of entries someone else keeps alive. `deposit_id` is capped too:
/// a registered custodian supplies it, but in Phase 1 custodian keys are
/// held by the backend, so registration is not the safeguard it sounds
/// like. 64 bytes fits a UUID, a bank reference or a hex SHA-256.
pub const MAX_REF_LEN: u32 = 64;

fn require_ref(r: &String) -> Result<(), Error> {
    if r.len() > MAX_REF_LEN {
        return Err(Error::RefTooLong);
    }
    Ok(())
}

/// **The pledge-eligibility rule.** Only scale-weighed receipts can be
/// collateral: bag-count estimates drift with moisture and fill level.
///
/// `lock` and `is_pledgeable` both go through here, so relaxing or tightening
/// the rule is a change to this function alone.
pub fn require_scale_weight(receipt: &Receipt) -> Result<(), Error> {
    if receipt.weight_source != WeightSource::Scale {
        return Err(Error::OnlyScaleEligibleForLock);
    }
    Ok(())
}

pub fn is_pledgeable(receipt: &Receipt) -> bool {
    receipt.status == Status::Active && require_scale_weight(receipt).is_ok()
}

/// The pledge on a receipt that is currently `Locked` or in `Recovery`.
///
/// A receipt that is `Active`, `Exited` or `Revoked` has no live pledge, so
/// it is `NotLocked`.
pub fn current_lock(receipt: &Receipt) -> Result<Lock, Error> {
    match (&receipt.status, &receipt.lock) {
        (Status::Locked | Status::Recovery, MaybeLock::Some(lock)) => Ok(lock.clone()),
        _ => Err(Error::NotLocked),
    }
}

/// Status check first, then the recorded lender's signature. Nothing is
/// written before both pass.
fn require_lender(receipt: &Receipt, accepted: &[Status]) -> Result<Lock, Error> {
    if !accepted.contains(&receipt.status) {
        return Err(Error::InvalidStatus);
    }
    let lock = current_lock(receipt).map_err(|_| Error::InvalidStatus)?;
    lock.lender.require_auth();
    Ok(lock)
}

fn load(env: &Env, receipt_id: u64) -> Result<Receipt, Error> {
    storage::get_receipt(env, receipt_id).ok_or(Error::ReceiptNotFound)
}

/// See `ReceiptContract::lock`.
pub fn lock(
    env: &Env,
    receipt_id: u64,
    lender: Address,
    kg: u64,
    loan_ref: String,
    due_date: u64,
    grace_period: u64,
) -> Result<u64, Error> {
    let receipt = load(env, receipt_id)?;

    // Checked before authorising: with lender == owner there is only one
    // party, and rejecting early grants nothing.
    if lender == receipt.owner {
        return Err(Error::LenderIsOwner);
    }

    // Both signatures before any validation that could short-circuit them.
    lender.require_auth();
    receipt.owner.require_auth();

    if receipt.status != Status::Active {
        return Err(Error::InvalidStatus);
    }
    require_scale_weight(&receipt)?;

    let now = env.ledger().timestamp();
    if due_date <= now || due_date.checked_add(grace_period).is_none() {
        return Err(Error::InvalidDueDate);
    }
    if kg == 0 || kg > receipt.weight_kg {
        return Err(Error::InvalidSplitAmount);
    }
    require_ref(&loan_ref)?;

    let mut locking = if kg == receipt.weight_kg {
        receipt
    } else {
        split::split_receipt(env, &receipt, kg)?.taken
    };

    locking.status = Status::Locked;
    locking.lock = MaybeLock::Some(Lock {
        lender: lender.clone(),
        loan_ref: loan_ref.clone(),
        due_date,
        grace_period,
    });
    storage::set_receipt(env, &locking);
    storage::index_lender(env, &lender, locking.receipt_id);
    storage::bump_instance(env);

    events::Locked {
        receipt_id: locking.receipt_id,
        lender,
        owner: locking.owner.clone(),
        warehouse_id: locking.warehouse_id.clone(),
        kg: locking.weight_kg,
        loan_ref,
        due_date,
        grace_period,
        locked_at: now,
    }
    .publish(env);
    Ok(locking.receipt_id)
}

/// See `ReceiptContract::unlock`.
pub fn unlock(env: &Env, receipt_id: u64, kg: u64) -> Result<u64, Error> {
    let receipt = load(env, receipt_id)?;
    let lock = require_lender(&receipt, &[Status::Locked, Status::Recovery])?;

    if kg == 0 || kg > receipt.weight_kg {
        return Err(Error::InvalidSplitAmount);
    }

    let now = env.ledger().timestamp();
    let from_recovery = receipt.status == Status::Recovery;

    let (mut released, remaining_receipt_id) = if kg == receipt.weight_kg {
        (receipt, None)
    } else {
        let parts = split::split_receipt(env, &receipt, kg)?;
        (parts.taken, Some(parts.remainder.receipt_id))
    };

    // The released portion goes back to whoever owns it — never the lender.
    released.status = Status::Active;
    released.lock = MaybeLock::None;
    if let MaybeRecovery::Some(mut recovery) = released.recovery.clone() {
        recovery.resolution = RecoveryResolution::Repaid;
        recovery.resolved_at = Some(now);
        released.recovery_history.push_back(recovery);
        released.recovery = MaybeRecovery::None;
    }
    storage::set_receipt(env, &released);
    storage::bump_instance(env);

    events::Unlocked {
        receipt_id: released.receipt_id,
        lender: lock.lender,
        owner: released.owner.clone(),
        warehouse_id: released.warehouse_id.clone(),
        loan_ref: lock.loan_ref,
        kg: released.weight_kg,
        partial: remaining_receipt_id.is_some(),
        remaining_receipt_id,
        from_recovery,
        unlocked_at: now,
    }
    .publish(env);
    Ok(released.receipt_id)
}

/// See `ReceiptContract::extend_due`.
pub fn extend_due(env: &Env, receipt_id: u64, new_due_date: u64) -> Result<(), Error> {
    let mut receipt = load(env, receipt_id)?;
    let mut lock = require_lender(&receipt, &[Status::Locked])?;

    if new_due_date <= lock.due_date {
        return Err(Error::DueDateNotLater);
    }
    if new_due_date.checked_add(lock.grace_period).is_none() {
        return Err(Error::InvalidDueDate);
    }

    let old_due_date = lock.due_date;
    lock.due_date = new_due_date;
    receipt.lock = MaybeLock::Some(lock.clone());
    storage::set_receipt(env, &receipt);
    storage::bump_instance(env);

    events::DueExtended {
        receipt_id,
        lender: lock.lender,
        owner: receipt.owner,
        warehouse_id: receipt.warehouse_id,
        loan_ref: lock.loan_ref,
        kg: receipt.weight_kg,
        old_due_date,
        new_due_date,
        grace_period: lock.grace_period,
        extended_at: env.ledger().timestamp(),
    }
    .publish(env);
    Ok(())
}

/// See `ReceiptContract::start_recovery`.
pub fn start_recovery(env: &Env, receipt_id: u64) -> Result<(), Error> {
    let mut receipt = load(env, receipt_id)?;
    let lock = require_lender(&receipt, &[Status::Locked])?;

    let now = env.ledger().timestamp();
    // `lock` and `extend_due` both reject an overflowing sum, so this cannot
    // saturate in practice; saturating keeps the arithmetic total regardless.
    let deadline = lock.due_date.saturating_add(lock.grace_period);
    if now <= deadline {
        return Err(Error::RecoveryNotYetDue);
    }

    receipt.status = Status::Recovery;
    receipt.recovery = MaybeRecovery::Some(Recovery {
        lender: lock.lender.clone(),
        loan_ref: lock.loan_ref.clone(),
        owner: receipt.owner.clone(),
        started_at: now,
        resolution: RecoveryResolution::Open,
        resolved_at: None,
        sale_id: None,
        buyer: None,
        sale_ref: None,
    });
    storage::set_receipt(env, &receipt);
    storage::bump_instance(env);

    events::RecoveryStarted {
        receipt_id,
        lender: lock.lender,
        warehouse_id: receipt.warehouse_id,
        owner: receipt.owner,
        loan_ref: lock.loan_ref,
        kg: receipt.weight_kg,
        due_date: lock.due_date,
        grace_period: lock.grace_period,
        started_at: now,
    }
    .publish(env);
    Ok(())
}

/// See `ReceiptContract::complete_recovery`.
pub fn complete_recovery(
    env: &Env,
    receipt_id: u64,
    operator: Address,
    buyer: Address,
    sale_ref: String,
) -> Result<u64, Error> {
    let mut receipt = load(env, receipt_id)?;
    if receipt.status != Status::Recovery {
        return Err(Error::InvalidStatus);
    }
    let MaybeRecovery::Some(mut recovery) = receipt.recovery.clone() else {
        return Err(Error::InvalidStatus);
    };

    operator.require_auth();
    let lock = require_lender(&receipt, &[Status::Recovery])?;

    if !admin::is_recovery_operator(env, &operator) {
        return Err(Error::RecoveryOperatorNotRegistered);
    }
    require_ref(&sale_ref)?;

    let now = env.ledger().timestamp();
    let sale = Sale {
        data_version: DATA_VERSION,
        sale_id: storage::next_sale_id(env),
        receipt_id,
        warehouse_id: receipt.warehouse_id.clone(),
        weight_kg: receipt.weight_kg,
        lender: lock.lender.clone(),
        loan_ref: lock.loan_ref.clone(),
        operator: operator.clone(),
        previous_owner: receipt.owner.clone(),
        buyer: buyer.clone(),
        sale_ref: sale_ref.clone(),
        started_at: recovery.started_at,
        completed_at: now,
        settlement_ref: None,
        settled_at: None,
    };

    recovery.resolution = RecoveryResolution::Completed;
    recovery.resolved_at = Some(now);
    recovery.sale_id = Some(sale.sale_id);
    recovery.buyer = Some(buyer.clone());
    recovery.sale_ref = Some(sale_ref.clone());

    receipt.recovery_history.push_back(recovery);
    receipt.recovery = MaybeRecovery::None;
    receipt.lock = MaybeLock::None;
    receipt.owner = buyer.clone();
    receipt.status = Status::Active;
    storage::set_receipt(env, &receipt);
    storage::set_sale(env, &sale);
    storage::push_unsettled_sale(env, sale.sale_id);
    storage::index_lender_sale(env, &sale.lender, sale.sale_id);
    storage::index_owner(env, &buyer, receipt_id);
    storage::bump_instance(env);

    events::RecoveryCompleted {
        receipt_id,
        lender: sale.lender,
        sale_id: sale.sale_id,
        operator,
        previous_owner: sale.previous_owner,
        buyer,
        warehouse_id: sale.warehouse_id,
        loan_ref: sale.loan_ref,
        kg: sale.weight_kg,
        sale_ref,
        started_at: sale.started_at,
        completed_at: now,
    }
    .publish(env);
    Ok(sale.sale_id)
}

/// See `ReceiptContract::record_settlement`.
///
/// The settlement is written to the `Sale`, not the receipt: by the time the
/// bank settles, the buyer may already have split or exited the receipt,
/// and every descendant's history resolves the settlement through
/// `sale_id`.
pub fn record_settlement(
    env: &Env,
    sale_id: u64,
    operator: Address,
    settlement_ref: String,
) -> Result<(), Error> {
    let mut sale = storage::get_sale(env, sale_id).ok_or(Error::SaleNotFound)?;

    operator.require_auth();
    sale.lender.require_auth();

    if !admin::is_recovery_operator(env, &operator) {
        return Err(Error::RecoveryOperatorNotRegistered);
    }
    if sale.settlement_ref.is_some() {
        return Err(Error::RecoveryAlreadySettled);
    }
    require_ref(&settlement_ref)?;

    let now = env.ledger().timestamp();
    sale.settlement_ref = Some(settlement_ref.clone());
    sale.settled_at = Some(now);
    storage::set_sale(env, &sale);
    storage::remove_unsettled_sale(env, sale_id);
    storage::bump_instance(env);

    events::SettlementRecorded {
        sale_id,
        lender: sale.lender,
        receipt_id: sale.receipt_id,
        operator,
        previous_owner: sale.previous_owner,
        buyer: sale.buyer,
        warehouse_id: sale.warehouse_id,
        loan_ref: sale.loan_ref,
        kg: sale.weight_kg,
        sale_ref: sale.sale_ref,
        settlement_ref,
        settled_at: now,
    }
    .publish(env);
    Ok(())
}
