use soroban_sdk::{contracttype, Address, Env, String, Symbol, Vec};

use crate::types::Receipt;

/// ~5s per ledger.
pub const DAY_IN_LEDGERS: u32 = 17_280;

/// Bump an entry once it has less than 30 days of life left...
pub const ENTRY_TTL_THRESHOLD: u32 = 30 * DAY_IN_LEDGERS;
/// ...back up to 175 days.
///
/// The network ceiling (`max_entry_ttl`) is 3,110,400 ledgers = 180 days, so
/// **no single bump can cover a receipt's full 6-12 month life**. 175 days
/// leaves five days of headroom, so a validator vote that lowers the ceiling
/// slightly cannot turn every write into a trap. See the README on rent.
pub const ENTRY_TTL_EXTEND_TO: u32 = 175 * DAY_IN_LEDGERS;

/// Instance storage (admin, id counter) is bumped on every state-changing call.
pub const INSTANCE_TTL_THRESHOLD: u32 = 30 * DAY_IN_LEDGERS;
pub const INSTANCE_TTL_EXTEND_TO: u32 = 175 * DAY_IN_LEDGERS;

/// Receipt ids per index page. A page of 64 `u64`s is ~0.5KB, far under the
/// 64KB per-entry ceiling, and a mint rewrites only the page it lands in
/// rather than the warehouse's whole history.
pub const INDEX_PAGE_SIZE: u32 = 64;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    // --- instance ---
    Admin,
    NextReceiptId,

    // --- registries (persistent; value is always `()`, presence is the fact) ---
    Warehouse(Symbol),
    /// Custodians are registered per warehouse, not globally.
    Custodian(Symbol, Address),
    Verifier(Address),
    /// Stored now, consumed by `complete_recovery` next session.
    RecoveryOperator(Address),

    // --- receipts ---
    Receipt(u64),
    /// `deposit_id -> receipt_id`, the duplicate-mint guard.
    DepositIndex(String),

    // --- paged secondary indexes ---
    OwnerIndex(Address, u32),
    OwnerPages(Address),
    WarehouseIndex(Symbol, u32),
    WarehousePages(Symbol),
}

// --- TTL -------------------------------------------------------------------

pub fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

/// Bump any persistent entry. `extend_ttl` is floor-only and idempotent: it
/// is a no-op unless the remaining TTL is already below the threshold, and it
/// never shortens a TTL.
pub fn bump(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
}

pub fn bump_receipt(env: &Env, receipt_id: u64) {
    bump(env, &DataKey::Receipt(receipt_id));
}

// --- receipts --------------------------------------------------------------

pub fn get_receipt(env: &Env, receipt_id: u64) -> Option<Receipt> {
    env.storage()
        .persistent()
        .get(&DataKey::Receipt(receipt_id))
}

pub fn set_receipt(env: &Env, receipt: &Receipt) {
    let key = DataKey::Receipt(receipt.receipt_id);
    env.storage().persistent().set(&key, receipt);
    bump(env, &key);
}

pub fn has_deposit_id(env: &Env, deposit_id: &String) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::DepositIndex(deposit_id.clone()))
}

pub fn set_deposit_index(env: &Env, deposit_id: &String, receipt_id: u64) {
    let key = DataKey::DepositIndex(deposit_id.clone());
    env.storage().persistent().set(&key, &receipt_id);
    bump(env, &key);
}

pub fn bump_deposit_index(env: &Env, deposit_id: &String) {
    bump(env, &DataKey::DepositIndex(deposit_id.clone()));
}

/// Ids start at 1, so `0` is never a valid receipt id.
pub fn next_receipt_id(env: &Env) -> u64 {
    let key = DataKey::NextReceiptId;
    let next: u64 = env.storage().instance().get(&key).unwrap_or(0) + 1;
    env.storage().instance().set(&key, &next);
    next
}

// --- paged secondary indexes -----------------------------------------------

/// Append `receipt_id` to the last page, starting a new page when the current
/// one is full. Pages are append-only: a receipt is never removed from an
/// index, because `Split`, `Exited` and `Revoked` receipts stay readable as
/// audit records. Callers filter by `status`.
fn push_index(
    env: &Env,
    pages_key: &DataKey,
    mut page_key: impl FnMut(u32) -> DataKey,
    receipt_id: u64,
) {
    let store = env.storage().persistent();
    let pages: u32 = store.get(pages_key).unwrap_or(0);

    let (page, mut ids) = if pages == 0 {
        (0u32, Vec::new(env))
    } else {
        let last = pages - 1;
        let ids: Vec<u64> = store.get(&page_key(last)).unwrap_or(Vec::new(env));
        if ids.len() >= INDEX_PAGE_SIZE {
            (pages, Vec::new(env))
        } else {
            (last, ids)
        }
    };

    ids.push_back(receipt_id);

    let key = page_key(page);
    store.set(&key, &ids);
    bump(env, &key);
    store.set(pages_key, &(page + 1));
    bump(env, pages_key);
}

pub fn index_owner(env: &Env, owner: &Address, receipt_id: u64) {
    push_index(
        env,
        &DataKey::OwnerPages(owner.clone()),
        |page| DataKey::OwnerIndex(owner.clone(), page),
        receipt_id,
    );
}

pub fn index_warehouse(env: &Env, warehouse_id: &Symbol, receipt_id: u64) {
    push_index(
        env,
        &DataKey::WarehousePages(warehouse_id.clone()),
        |page| DataKey::WarehouseIndex(warehouse_id.clone(), page),
        receipt_id,
    );
}

pub fn owner_page(env: &Env, owner: &Address, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::OwnerIndex(owner.clone(), page))
        .unwrap_or(Vec::new(env))
}

pub fn warehouse_page(env: &Env, warehouse_id: &Symbol, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::WarehouseIndex(warehouse_id.clone(), page))
        .unwrap_or(Vec::new(env))
}

pub fn owner_pages(env: &Env, owner: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::OwnerPages(owner.clone()))
        .unwrap_or(0)
}

pub fn warehouse_pages(env: &Env, warehouse_id: &Symbol) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::WarehousePages(warehouse_id.clone()))
        .unwrap_or(0)
}
