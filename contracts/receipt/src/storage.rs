use soroban_sdk::{contracttype, Address, Env, String, Symbol, Vec};

use crate::layout::{self, StoredReceipt, StoredSale};
use crate::types::{Index, Receipt, Sale};

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
    NextSaleId,

    // --- registries (persistent; value is always `()`, presence is the fact) ---
    Warehouse(Symbol),
    /// Custodians are registered per warehouse, not globally.
    Custodian(Symbol, Address),
    Verifier(Address),
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
    LenderIndex(Address, u32),
    LenderPages(Address),

    // --- recovery sales ---
    Sale(u64),
    /// `sale_id`s awaiting `record_settlement`, oldest first.
    UnsettledSales,
    /// Every sale whose recovery `lender` started, paged like the receipt
    /// indexes but holding `sale_id`s.
    LenderSaleIndex(Address, u32),
    LenderSalePages(Address),
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

/// Bump `key` only if it exists. `extend_ttl` on a missing key traps, and
/// keep-alive calls should not fail over an index page that was never
/// written.
pub fn bump_if_present(env: &Env, key: &DataKey) {
    if env.storage().persistent().has(key) {
        bump(env, key);
    }
}

// --- receipts --------------------------------------------------------------

/// Receipts and sales are stored in the compact layout in `layout.rs`,
/// and converted at this boundary only.
pub fn get_receipt(env: &Env, receipt_id: u64) -> Option<Receipt> {
    env.storage()
        .persistent()
        .get::<_, StoredReceipt>(&DataKey::Receipt(receipt_id))
        .map(|stored| layout::load_receipt(env, receipt_id, stored))
}

pub fn set_receipt(env: &Env, receipt: &Receipt) {
    let key = DataKey::Receipt(receipt.receipt_id);
    env.storage()
        .persistent()
        .set(&key, &layout::store_receipt(env, receipt));
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

// --- recovery sales --------------------------------------------------------

/// Ids start at 1, like receipt ids.
pub fn next_sale_id(env: &Env) -> u64 {
    let key = DataKey::NextSaleId;
    let next: u64 = env.storage().instance().get(&key).unwrap_or(0) + 1;
    env.storage().instance().set(&key, &next);
    next
}

pub fn get_sale(env: &Env, sale_id: u64) -> Option<Sale> {
    env.storage()
        .persistent()
        .get::<_, StoredSale>(&DataKey::Sale(sale_id))
        .map(|stored| layout::load_sale(sale_id, stored))
}

pub fn set_sale(env: &Env, sale: &Sale) {
    let key = DataKey::Sale(sale.sale_id);
    env.storage()
        .persistent()
        .set(&key, &layout::store_sale(sale));
    bump(env, &key);
}

pub fn bump_sale(env: &Env, sale_id: u64) {
    bump_if_present(env, &DataKey::Sale(sale_id));
}

/// Unsettled sales are few and short-lived — each is removed as soon as the
/// bank settles — so one list entry is enough; it is not an audit index.
pub fn unsettled_sales(env: &Env) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::UnsettledSales)
        .unwrap_or(Vec::new(env))
}

fn set_unsettled_sales(env: &Env, ids: &Vec<u64>) {
    let key = DataKey::UnsettledSales;
    env.storage().persistent().set(&key, ids);
    bump(env, &key);
}

pub fn push_unsettled_sale(env: &Env, sale_id: u64) {
    let mut ids = unsettled_sales(env);
    ids.push_back(sale_id);
    set_unsettled_sales(env, &ids);
}

pub fn remove_unsettled_sale(env: &Env, sale_id: u64) {
    let mut ids = unsettled_sales(env);
    if let Some(i) = ids.first_index_of(sale_id) {
        ids.remove(i);
        set_unsettled_sales(env, &ids);
    }
}

// --- paged secondary indexes -----------------------------------------------

/// Append `id` (a receipt id, or a sale id for the lender-sale index) to the
/// last page, starting a new page when the current one is full. Pages are
/// append-only: a receipt is never removed from an index, because `Exited`
/// and `Revoked` receipts stay readable (restored on read once archived).
/// Callers filter by `status`.
///
/// A full page is never written again, so nothing here keeps it alive past
/// `ENTRY_TTL_EXTEND_TO`; `extend_index_ttl` exists for that.
fn push_index(env: &Env, pages_key: &DataKey, mut page_key: impl FnMut(u32) -> DataKey, id: u64) {
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

    ids.push_back(id);

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

/// Every receipt ever pledged to `lender`, including one split off a pledged
/// receipt.
/// A receipt re-pledged to the same lender after a full unlock appears twice;
/// readers filter on `status` and `lock`.
pub fn index_lender(env: &Env, lender: &Address, receipt_id: u64) {
    push_index(
        env,
        &DataKey::LenderPages(lender.clone()),
        |page| DataKey::LenderIndex(lender.clone(), page),
        receipt_id,
    );
}

/// Every sale from a recovery `lender` started, so a lender can find its
/// settlements without knowing receipt ids.
pub fn index_lender_sale(env: &Env, lender: &Address, sale_id: u64) {
    push_index(
        env,
        &DataKey::LenderSalePages(lender.clone()),
        |page| DataKey::LenderSaleIndex(lender.clone(), page),
        sale_id,
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

pub fn lender_page(env: &Env, lender: &Address, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::LenderIndex(lender.clone(), page))
        .unwrap_or(Vec::new(env))
}

pub fn lender_pages(env: &Env, lender: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::LenderPages(lender.clone()))
        .unwrap_or(0)
}

pub fn lender_sale_page(env: &Env, lender: &Address, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::LenderSaleIndex(lender.clone(), page))
        .unwrap_or(Vec::new(env))
}

pub fn lender_sale_pages(env: &Env, lender: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::LenderSalePages(lender.clone()))
        .unwrap_or(0)
}

/// The page-count key and the key of page `page` for one paged index.
pub fn index_keys(index: &Index, page: u32) -> (DataKey, DataKey) {
    match index {
        Index::Owner(a) => (
            DataKey::OwnerPages(a.clone()),
            DataKey::OwnerIndex(a.clone(), page),
        ),
        Index::Warehouse(w) => (
            DataKey::WarehousePages(w.clone()),
            DataKey::WarehouseIndex(w.clone(), page),
        ),
        Index::Lender(a) => (
            DataKey::LenderPages(a.clone()),
            DataKey::LenderIndex(a.clone(), page),
        ),
        Index::LenderSales(a) => (
            DataKey::LenderSalePages(a.clone()),
            DataKey::LenderSaleIndex(a.clone(), page),
        ),
    }
}
