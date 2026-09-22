use soroban_sdk::{contracttype, Address, Env, String, Symbol};

use crate::types::{Lock, Receipt, Recovery};

/// Persistent entries (receipts, registries) live ~6-12 months; bump well
/// before expiry so the daily TTL-extension job (design doc §7.2) has room.
pub const RECEIPT_TTL_THRESHOLD: u32 = 17_280 * 30; // ~30 days of ledgers
pub const RECEIPT_TTL_EXTEND_TO: u32 = 17_280 * 180; // ~180 days of ledgers

/// Instance storage (admin, registries) is bumped alongside every call.
pub const INSTANCE_TTL_THRESHOLD: u32 = 17_280 * 30;
pub const INSTANCE_TTL_EXTEND_TO: u32 = 17_280 * 180;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    NextReceiptId,
    Receipt(u64),
    /// Present only while the receipt's status is LOCKED or RECOVERY.
    Lock(u64),
    /// Present only while the receipt's status is RECOVERY.
    Recovery(u64),
    DepositIndex(String),
    /// Custodians are registered per warehouse.
    Custodian(Symbol, Address),
    Verifier(Address),
    RecoveryOperator(Address),
}

pub fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

pub fn bump_receipt(env: &Env, receipt_id: u64) {
    let key = DataKey::Receipt(receipt_id);
    env.storage()
        .persistent()
        .extend_ttl(&key, RECEIPT_TTL_THRESHOLD, RECEIPT_TTL_EXTEND_TO);
}

pub fn has_receipt(env: &Env, receipt_id: u64) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::Receipt(receipt_id))
}

pub fn get_receipt(env: &Env, receipt_id: u64) -> Option<Receipt> {
    env.storage()
        .persistent()
        .get(&DataKey::Receipt(receipt_id))
}

pub fn set_receipt(env: &Env, receipt: &Receipt) {
    let key = DataKey::Receipt(receipt.receipt_id);
    env.storage().persistent().set(&key, receipt);
    bump_receipt(env, receipt.receipt_id);
}

pub fn get_lock(env: &Env, receipt_id: u64) -> Option<Lock> {
    env.storage().persistent().get(&DataKey::Lock(receipt_id))
}

pub fn set_lock(env: &Env, receipt_id: u64, lock: &Lock) {
    let key = DataKey::Lock(receipt_id);
    env.storage().persistent().set(&key, lock);
    env.storage()
        .persistent()
        .extend_ttl(&key, RECEIPT_TTL_THRESHOLD, RECEIPT_TTL_EXTEND_TO);
}

pub fn remove_lock(env: &Env, receipt_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::Lock(receipt_id));
}

pub fn get_recovery(env: &Env, receipt_id: u64) -> Option<Recovery> {
    env.storage()
        .persistent()
        .get(&DataKey::Recovery(receipt_id))
}

pub fn set_recovery(env: &Env, receipt_id: u64, recovery: &Recovery) {
    let key = DataKey::Recovery(receipt_id);
    env.storage().persistent().set(&key, recovery);
    env.storage()
        .persistent()
        .extend_ttl(&key, RECEIPT_TTL_THRESHOLD, RECEIPT_TTL_EXTEND_TO);
}

pub fn remove_recovery(env: &Env, receipt_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::Recovery(receipt_id));
}

pub fn has_deposit_id(env: &Env, deposit_id: &String) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::DepositIndex(deposit_id.clone()))
}

pub fn set_deposit_index(env: &Env, deposit_id: &String, receipt_id: u64) {
    let key = DataKey::DepositIndex(deposit_id.clone());
    env.storage().persistent().set(&key, &receipt_id);
    env.storage()
        .persistent()
        .extend_ttl(&key, RECEIPT_TTL_THRESHOLD, RECEIPT_TTL_EXTEND_TO);
}

pub fn next_receipt_id(env: &Env) -> u64 {
    let key = DataKey::NextReceiptId;
    let current: u64 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&key, &next);
    next
}
