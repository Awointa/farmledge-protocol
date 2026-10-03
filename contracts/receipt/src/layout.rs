//! How receipts and sales are laid out in storage.
//!
//! Rent is charged per byte stored, and a `#[contracttype]` struct with named
//! fields is stored as a map keyed by its field names: 404 of a receipt's 816
//! value bytes were names. So storage uses the positional records below,
//! while the public [`Receipt`] and [`Sale`] types — what reads return and
//! what the contract spec describes — are unchanged. Unit enums are stored
//! as `u32` codes, an absent lock or recovery as an empty `Vec`, and the id
//! is not stored at all because it is already in the key.
//!
//! **Field order and codes are the layout.** Changing either is a layout
//! change: bump [`DATA_VERSION`] and keep the old reader for old records.
//! See `docs/archival.md` for the measured sizes.

use soroban_sdk::{contracttype, panic_with_error, Address, Env, String, Symbol, Vec};

use crate::errors::Error;
use crate::types::{
    Lock, MaybeLock, MaybeRecovery, Receipt, Recovery, RecoveryResolution, Sale, Status,
    WeightSource, DATA_VERSION,
};

/// A [`Lock`]: lender, loan_ref, due_date, grace_period.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredLock(pub Address, pub String, pub u64, pub u64);

/// A [`Recovery`]: lender, loan_ref, owner, started_at, resolution code,
/// resolved_at, sale_id, buyer, sale_ref.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredRecovery(
    pub Address,
    pub String,
    pub Address,
    pub u64,
    pub u32,
    pub Option<u64>,
    pub Option<u64>,
    pub Option<Address>,
    pub Option<String>,
);

/// A [`Receipt`] without its `receipt_id`, in field order: data_version,
/// deposit_id, parent_id, commodity, grade, weight_kg, weight_source code,
/// bag_count, moisture_pct, warehouse_id, custodian, verifier, owner, status
/// code, lock (empty or one), recovery (empty or one), recovery_history,
/// deposit_date, exit_date, exit_weight_kg.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredReceipt(
    pub u32,
    pub String,
    pub Option<u64>,
    pub Symbol,
    pub Symbol,
    pub u64,
    pub u32,
    pub u32,
    pub u32,
    pub Symbol,
    pub Address,
    pub Address,
    pub Address,
    pub u32,
    pub Vec<StoredLock>,
    pub Vec<StoredRecovery>,
    pub Vec<StoredRecovery>,
    pub u64,
    pub Option<u64>,
    pub Option<u64>,
);

/// A [`Sale`] without its `sale_id`, in field order: data_version,
/// receipt_id, warehouse_id, weight_kg, lender, loan_ref, operator,
/// previous_owner, buyer, sale_ref, started_at, completed_at,
/// settlement_ref, settled_at.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSale(
    pub u32,
    pub u64,
    pub Symbol,
    pub u64,
    pub Address,
    pub String,
    pub Address,
    pub Address,
    pub Address,
    pub String,
    pub u64,
    pub u64,
    pub Option<String>,
    pub Option<u64>,
);

// --- codes -----------------------------------------------------------------

fn status_code(status: &Status) -> u32 {
    match status {
        Status::Active => 0,
        Status::Locked => 1,
        Status::Recovery => 2,
        Status::Exited => 3,
        Status::Revoked => 4,
    }
}

fn status_from(env: &Env, code: u32) -> Status {
    match code {
        0 => Status::Active,
        1 => Status::Locked,
        2 => Status::Recovery,
        3 => Status::Exited,
        4 => Status::Revoked,
        _ => panic_with_error!(env, Error::CorruptRecord),
    }
}

fn weight_source_code(source: &WeightSource) -> u32 {
    match source {
        WeightSource::BagEstimate => 0,
        WeightSource::Scale => 1,
    }
}

fn weight_source_from(env: &Env, code: u32) -> WeightSource {
    match code {
        0 => WeightSource::BagEstimate,
        1 => WeightSource::Scale,
        _ => panic_with_error!(env, Error::CorruptRecord),
    }
}

fn resolution_code(resolution: &RecoveryResolution) -> u32 {
    match resolution {
        RecoveryResolution::Open => 0,
        RecoveryResolution::Repaid => 1,
        RecoveryResolution::Completed => 2,
    }
}

fn resolution_from(env: &Env, code: u32) -> RecoveryResolution {
    match code {
        0 => RecoveryResolution::Open,
        1 => RecoveryResolution::Repaid,
        2 => RecoveryResolution::Completed,
        _ => panic_with_error!(env, Error::CorruptRecord),
    }
}

// --- records ---------------------------------------------------------------

fn store_recovery(r: &Recovery) -> StoredRecovery {
    StoredRecovery(
        r.lender.clone(),
        r.loan_ref.clone(),
        r.owner.clone(),
        r.started_at,
        resolution_code(&r.resolution),
        r.resolved_at,
        r.sale_id,
        r.buyer.clone(),
        r.sale_ref.clone(),
    )
}

fn load_recovery(env: &Env, r: StoredRecovery) -> Recovery {
    Recovery {
        lender: r.0,
        loan_ref: r.1,
        owner: r.2,
        started_at: r.3,
        resolution: resolution_from(env, r.4),
        resolved_at: r.5,
        sale_id: r.6,
        buyer: r.7,
        sale_ref: r.8,
    }
}

pub fn store_receipt(env: &Env, r: &Receipt) -> StoredReceipt {
    let mut lock = Vec::new(env);
    if let MaybeLock::Some(l) = &r.lock {
        lock.push_back(StoredLock(
            l.lender.clone(),
            l.loan_ref.clone(),
            l.due_date,
            l.grace_period,
        ));
    }
    let mut recovery = Vec::new(env);
    if let MaybeRecovery::Some(open) = &r.recovery {
        recovery.push_back(store_recovery(open));
    }
    let mut history = Vec::new(env);
    for past in r.recovery_history.iter() {
        history.push_back(store_recovery(&past));
    }
    StoredReceipt(
        DATA_VERSION,
        r.deposit_id.clone(),
        r.parent_id,
        r.commodity.clone(),
        r.grade.clone(),
        r.weight_kg,
        weight_source_code(&r.weight_source),
        r.bag_count,
        r.moisture_pct,
        r.warehouse_id.clone(),
        r.custodian.clone(),
        r.verifier.clone(),
        r.owner.clone(),
        status_code(&r.status),
        lock,
        recovery,
        history,
        r.deposit_date,
        r.exit_date,
        r.exit_weight_kg,
    )
}

pub fn load_receipt(env: &Env, receipt_id: u64, s: StoredReceipt) -> Receipt {
    let lock = match s.14.first() {
        Some(l) => MaybeLock::Some(Lock {
            lender: l.0,
            loan_ref: l.1,
            due_date: l.2,
            grace_period: l.3,
        }),
        None => MaybeLock::None,
    };
    let recovery = match s.15.first() {
        Some(r) => MaybeRecovery::Some(load_recovery(env, r)),
        None => MaybeRecovery::None,
    };
    let mut recovery_history = Vec::new(env);
    for past in s.16.iter() {
        recovery_history.push_back(load_recovery(env, past));
    }
    Receipt {
        data_version: s.0,
        receipt_id,
        deposit_id: s.1,
        parent_id: s.2,
        commodity: s.3,
        grade: s.4,
        weight_kg: s.5,
        weight_source: weight_source_from(env, s.6),
        bag_count: s.7,
        moisture_pct: s.8,
        warehouse_id: s.9,
        custodian: s.10,
        verifier: s.11,
        owner: s.12,
        status: status_from(env, s.13),
        lock,
        recovery,
        recovery_history,
        deposit_date: s.17,
        exit_date: s.18,
        exit_weight_kg: s.19,
    }
}

pub fn store_sale(s: &Sale) -> StoredSale {
    StoredSale(
        DATA_VERSION,
        s.receipt_id,
        s.warehouse_id.clone(),
        s.weight_kg,
        s.lender.clone(),
        s.loan_ref.clone(),
        s.operator.clone(),
        s.previous_owner.clone(),
        s.buyer.clone(),
        s.sale_ref.clone(),
        s.started_at,
        s.completed_at,
        s.settlement_ref.clone(),
        s.settled_at,
    )
}

pub fn load_sale(sale_id: u64, s: StoredSale) -> Sale {
    Sale {
        data_version: s.0,
        sale_id,
        receipt_id: s.1,
        warehouse_id: s.2,
        weight_kg: s.3,
        lender: s.4,
        loan_ref: s.5,
        operator: s.6,
        previous_owner: s.7,
        buyer: s.8,
        sale_ref: s.9,
        started_at: s.10,
        completed_at: s.11,
        settlement_ref: s.12,
        settled_at: s.13,
    }
}
