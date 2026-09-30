#![no_std]
//! Farmledge digital warehouse receipts.
//!
//! One contract for all commodities — commodity is a field on the receipt,
//! not a separate deployment.
//!
//! Covers the deposit-to-exit lifecycle (`mint`, `exit`, `revoke`,
//! `extend_ttl`) and the lending lifecycle (`lock`, `unlock`, `extend_due`,
//! `start_recovery`, `complete_recovery`, `record_settlement`). The legal status transitions are
//! listed on [`Status`]. There is deliberately no user-to-user transfer, and
//! no fees, pricing or settlement on-chain.
//!
//! Soroban has no implicit caller identity. Every address whose signature an
//! operation needs is an explicit parameter that gets `require_auth()`d;
//! privileged addresses (the admin) are loaded from storage instead, because
//! authorising a caller-supplied address proves only that the caller controls
//! it, not that it is allowed to do anything.

mod admin;
mod errors;
mod events;
mod lending;
mod split;
mod storage;
mod types;

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol, Vec};

pub use errors::Error;
pub use types::{
    Lock, MaybeLock, MaybeRecovery, Receipt, Recovery, RecoveryResolution, Sale, Status,
    WeightSource,
};

/// Upper bound on `moisture_pct`, in basis points of a percent (100.00%).
const MAX_MOISTURE_BPS: u32 = 10_000;

#[contract]
pub struct ReceiptContract;

#[contractimpl]
impl ReceiptContract {
    /// One-time setup. Rejects re-initialisation.
    ///
    /// `admin` will be a Stellar multisig account in production; the contract
    /// only ever calls `require_auth()` on it and implements no multisig
    /// logic of its own.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if admin::get_admin(&env).is_some() {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        admin::set_admin(&env, &admin);
        storage::bump_instance(&env);
        Ok(())
    }

    // --- lifecycle ---------------------------------------------------------

    /// Issues a receipt for a deposit. Requires **both** the custodian's and
    /// the verifier's authorisation: one party weighs and receives the grain,
    /// a second independently attests to it.
    ///
    /// The custodian must be registered for this specific warehouse. The
    /// farmer does not sign — they are the beneficiary, not an attester.
    ///
    /// Returns the new `receipt_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn mint(
        env: Env,
        custodian: Address,
        verifier: Address,
        deposit_id: String,
        farmer: Address,
        warehouse_id: Symbol,
        commodity: Symbol,
        grade: Symbol,
        weight_kg: u64,
        weight_source: WeightSource,
        bag_count: u32,
        moisture_pct: u32,
    ) -> Result<u64, Error> {
        // Both signatures first, so neither can be skipped by tripping a
        // later validation error.
        custodian.require_auth();
        verifier.require_auth();

        if !admin::is_warehouse(&env, &warehouse_id) {
            return Err(Error::WarehouseNotRegistered);
        }
        if !admin::is_custodian(&env, &warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        if !admin::is_verifier(&env, &verifier) {
            return Err(Error::VerifierNotRegistered);
        }

        if weight_kg == 0 {
            return Err(Error::InvalidWeight);
        }
        // A bag-estimated weight with no bags is not an estimate of anything.
        if weight_source == WeightSource::BagEstimate && bag_count == 0 {
            return Err(Error::InvalidWeight);
        }
        if moisture_pct > MAX_MOISTURE_BPS {
            return Err(Error::InvalidMoisture);
        }
        if storage::has_deposit_id(&env, &deposit_id) {
            return Err(Error::DuplicateDepositId);
        }

        let receipt_id = storage::next_receipt_id(&env);
        let receipt = Receipt {
            receipt_id,
            deposit_id: deposit_id.clone(),
            parent_id: None,
            commodity,
            grade,
            weight_kg,
            weight_source,
            bag_count,
            moisture_pct,
            warehouse_id: warehouse_id.clone(),
            custodian,
            verifier,
            owner: farmer.clone(),
            status: Status::Active,
            lock: MaybeLock::None,
            recovery: MaybeRecovery::None,
            recovery_history: Vec::new(&env),
            deposit_date: env.ledger().timestamp(),
            exit_date: None,
            exit_weight_kg: None,
        };

        storage::set_receipt(&env, &receipt);
        storage::set_deposit_index(&env, &deposit_id, receipt_id);
        storage::index_owner(&env, &farmer, receipt_id);
        storage::index_warehouse(&env, &warehouse_id, receipt_id);
        storage::bump_instance(&env);

        events::Minted {
            receipt_id,
            warehouse_id: receipt.warehouse_id,
            deposit_id: receipt.deposit_id,
            owner: receipt.owner,
            commodity: receipt.commodity,
            grade: receipt.grade,
            weight_kg: receipt.weight_kg,
            weight_source: receipt.weight_source,
            bag_count: receipt.bag_count,
            moisture_pct: receipt.moisture_pct,
            custodian: receipt.custodian,
            verifier: receipt.verifier,
            deposit_date: receipt.deposit_date,
        }
        .publish(&env);
        Ok(receipt_id)
    }

    /// Releases grain from the warehouse. Requires **both** a custodian
    /// registered for the receipt's warehouse and the receipt's owner.
    ///
    /// `kg` is how much of the receipt's book weight is being retired;
    /// `exit_weight_kg` is what the grain actually weighed on the way out.
    /// The two differ by shrinkage, which is the point of recording both —
    /// `weight_kg` stays the authoritative quantity so that shrinkage never
    /// breaks the split conservation invariant.
    ///
    /// If `kg` is less than the receipt's weight the receipt is split first,
    /// and the remainder stays `Active`. Only `Active` receipts can exit.
    ///
    /// Returns the id of the receipt that was retired — the receipt itself on
    /// a full exit, or the exited child on a partial one.
    pub fn exit(
        env: Env,
        receipt_id: u64,
        custodian: Address,
        kg: u64,
        exit_weight_kg: u64,
    ) -> Result<u64, Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;

        // Any custodian registered for this warehouse may sign an exit, not
        // only the one who signed the original mint.
        custodian.require_auth();
        receipt.owner.require_auth();

        if !admin::is_custodian(&env, &receipt.warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        if receipt.status != Status::Active {
            return Err(Error::InvalidStatus);
        }
        if exit_weight_kg == 0 {
            return Err(Error::InvalidExitWeight);
        }
        if kg == 0 || kg > receipt.weight_kg {
            return Err(Error::InvalidSplitAmount);
        }

        let mut exiting = if kg == receipt.weight_kg {
            receipt
        } else {
            split::split_receipt(&env, &receipt, kg)?.taken
        };

        exiting.status = Status::Exited;
        exiting.exit_date = Some(env.ledger().timestamp());
        exiting.exit_weight_kg = Some(exit_weight_kg);
        storage::set_receipt(&env, &exiting);
        storage::bump_instance(&env);

        events::Exited {
            receipt_id: exiting.receipt_id,
            warehouse_id: exiting.warehouse_id,
            owner: exiting.owner,
            custodian,
            weight_kg: exiting.weight_kg,
            bag_count: exiting.bag_count,
            exit_weight_kg,
            exit_date: env.ledger().timestamp(),
        }
        .publish(&env);
        Ok(exiting.receipt_id)
    }

    /// Voids a receipt. Admin only, and only from `Active` — a receipt whose
    /// grain has already left, or that is already void, is not revocable.
    /// Neither is a pledged one: to void a `Locked` or `Recovery` receipt the
    /// lender unlocks it first, so a lender never has collateral disappear
    /// from under a live loan without having signed for it.
    ///
    /// Intended for proven fraudulent mints. `reason` is recorded in the
    /// event, not in storage.
    pub fn revoke(env: Env, receipt_id: u64, reason: String) -> Result<(), Error> {
        admin::require_admin(&env)?;

        let mut receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;
        if receipt.status != Status::Active {
            return Err(Error::InvalidStatus);
        }

        receipt.status = Status::Revoked;
        storage::set_receipt(&env, &receipt);
        storage::bump_instance(&env);

        events::Revoked {
            receipt_id,
            warehouse_id: receipt.warehouse_id,
            owner: receipt.owner,
            weight_kg: receipt.weight_kg,
            reason,
            revoked_at: env.ledger().timestamp(),
        }
        .publish(&env);
        Ok(())
    }

    /// Pushes a receipt's storage rent out. Callable by **anyone**, with no
    /// authorisation, so a keep-alive job can run unprivileged — and so can
    /// anyone else with an interest in the receipt surviving.
    ///
    /// Also bumps the receipt's `deposit_id` guard entry, keeping the
    /// duplicate-mint protection alive alongside the receipt it protects, and
    /// any `Sale` records its recovery history refers to.
    pub fn extend_ttl(env: Env, receipt_id: u64) -> Result<(), Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;

        storage::bump_receipt(&env, receipt_id);
        storage::bump_deposit_index(&env, &receipt.deposit_id);
        // Sale records live in their own entries; keep any this receipt's
        // history points at alive alongside it.
        for recovery in receipt.recovery_history.iter() {
            if let Some(sale_id) = recovery.sale_id {
                storage::bump_sale(&env, sale_id);
            }
        }

        events::TtlExtended { receipt_id }.publish(&env);
        Ok(())
    }

    // --- lending -----------------------------------------------------------

    /// Pledges `kg` of an `Active`, scale-weighed receipt to `lender`.
    /// Requires **both** the lender's and the owner's authorisation, and
    /// `lender != owner`.
    ///
    /// If `kg` is less than the receipt's weight the receipt is split first:
    /// only that portion is locked and the remainder stays `Active` under the
    /// owner. `due_date` (ledger timestamp) must be in the future;
    /// `grace_period` is seconds after it before recovery may start.
    ///
    /// Returns the id of the locked receipt — the receipt itself on a full
    /// lock, or the locked child on a partial one.
    pub fn lock(
        env: Env,
        receipt_id: u64,
        lender: Address,
        kg: u64,
        loan_ref: String,
        due_date: u64,
        grace_period: u64,
    ) -> Result<u64, Error> {
        lending::lock(
            &env,
            receipt_id,
            lender,
            kg,
            loan_ref,
            due_date,
            grace_period,
        )
    }

    /// Releases `kg` from a `Locked` or `Recovery` receipt back to its owner.
    /// Only the lender recorded in the lock may sign.
    ///
    /// A partial unlock splits: the released portion is `Active`, the rest
    /// stays `Locked` / in `Recovery` under the same lock. Unlocking from
    /// `Recovery` records the recovery as `Repaid` in `recovery_history`.
    ///
    /// Returns the id of the released receipt.
    pub fn unlock(env: Env, receipt_id: u64, kg: u64) -> Result<u64, Error> {
        lending::unlock(&env, receipt_id, kg)
    }

    /// Moves a `Locked` receipt's due date strictly later. Lender only.
    pub fn extend_due(env: Env, receipt_id: u64, new_due_date: u64) -> Result<(), Error> {
        lending::extend_due(&env, receipt_id, new_due_date)
    }

    /// `Locked -> Recovery`, once the ledger time is strictly past
    /// `due_date + grace_period`. Lender only.
    pub fn start_recovery(env: Env, receipt_id: u64) -> Result<(), Error> {
        lending::start_recovery(&env, receipt_id)
    }

    /// Records the escrow sale of a receipt in `Recovery`. Requires **both** a
    /// registered recovery operator and the recorded lender.
    ///
    /// `sale_ref` is the escrow deposit reference proving the buyer paid.
    /// Ownership passes to `buyer`, the lock is cleared and the receipt is
    /// `Active` again — the buyer then exits it (or pledges it) like any
    /// other. The buyer does not sign: like the farmer at mint, they are the
    /// beneficiary.
    ///
    /// Creates a canonical `Sale` record, appends a `Completed` recovery
    /// pointing at it to `recovery_history`, and queues the sale for
    /// `record_settlement`. Returns the `sale_id`.
    pub fn complete_recovery(
        env: Env,
        receipt_id: u64,
        operator: Address,
        buyer: Address,
        sale_ref: String,
    ) -> Result<u64, Error> {
        lending::complete_recovery(&env, receipt_id, operator, buyer, sale_ref)
    }

    /// Records the bank's settlement of a completed sale — the payout to the
    /// lender and any surplus to the farmer. Requires **both** a registered
    /// recovery operator and the sale's lender. Write-once.
    pub fn record_settlement(
        env: Env,
        sale_id: u64,
        operator: Address,
        settlement_ref: String,
    ) -> Result<(), Error> {
        lending::record_settlement(&env, sale_id, operator, settlement_ref)
    }

    // --- reads -------------------------------------------------------------

    /// The live pledge on a `Locked` or `Recovery` receipt; `NotLocked`
    /// otherwise.
    pub fn get_lock(env: Env, receipt_id: u64) -> Result<Lock, Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;
        lending::current_lock(&receipt)
    }

    /// Whether `lock` would accept this receipt on status and weight source.
    pub fn is_pledgeable(env: Env, receipt_id: u64) -> Result<bool, Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;
        Ok(lending::is_pledgeable(&receipt))
    }

    /// One page of receipts ever pledged to `lender`, 64 per page. Includes
    /// released, recovered and split records; filter on `status` and `lock`
    /// for live collateral.
    pub fn get_receipts_by_lender(env: Env, lender: Address, page: u32) -> Vec<Receipt> {
        let ids = storage::lender_page(&env, &lender, page);
        Self::collect(&env, ids)
    }

    /// Number of pages `get_receipts_by_lender` will serve.
    pub fn lender_page_count(env: Env, lender: Address) -> u32 {
        storage::lender_pages(&env, &lender)
    }

    pub fn get_sale(env: Env, sale_id: u64) -> Result<Sale, Error> {
        storage::get_sale(&env, sale_id).ok_or(Error::SaleNotFound)
    }

    /// Completed recoveries still waiting for `record_settlement`, oldest
    /// first, 64 per page.
    pub fn get_unsettled_sales(env: Env, page: u32) -> Vec<Sale> {
        let ids = storage::unsettled_sales(&env);
        let start = page.saturating_mul(storage::INDEX_PAGE_SIZE);
        let end = start
            .saturating_add(storage::INDEX_PAGE_SIZE)
            .min(ids.len());
        let mut out = Vec::new(&env);
        for i in start..end {
            if let Some(sale) = storage::get_sale(&env, ids.get_unchecked(i)) {
                out.push_back(sale);
            }
        }
        out
    }

    /// Number of sales awaiting settlement.
    pub fn unsettled_sale_count(env: Env) -> u32 {
        storage::unsettled_sales(&env).len()
    }

    pub fn get_receipt(env: Env, receipt_id: u64) -> Result<Receipt, Error> {
        storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)
    }

    /// One page of an owner's receipts, newest last, 64 per page.
    ///
    /// Paged because an unpaged read would blow the network's per-transaction
    /// ledger-read limit (200 entries) once a farmer has a few hundred
    /// receipts. Includes `Split`, `Exited` and `Revoked` records — they are
    /// the audit trail; filter on `status` for live holdings. A receipt sold
    /// in recovery stays listed under its previous owner too, so also filter
    /// on `owner`.
    pub fn get_receipts_by_owner(env: Env, owner: Address, page: u32) -> Vec<Receipt> {
        let ids = storage::owner_page(&env, &owner, page);
        Self::collect(&env, ids)
    }

    /// One page of a warehouse's receipts, newest last, 64 per page.
    pub fn get_receipts_by_warehouse(env: Env, warehouse_id: Symbol, page: u32) -> Vec<Receipt> {
        let ids = storage::warehouse_page(&env, &warehouse_id, page);
        Self::collect(&env, ids)
    }

    /// Number of pages `get_receipts_by_owner` will serve for `owner`.
    pub fn owner_page_count(env: Env, owner: Address) -> u32 {
        storage::owner_pages(&env, &owner)
    }

    /// Number of pages `get_receipts_by_warehouse` will serve.
    pub fn warehouse_page_count(env: Env, warehouse_id: Symbol) -> u32 {
        storage::warehouse_pages(&env, &warehouse_id)
    }

    // --- registries (admin only) -------------------------------------------

    pub fn add_warehouse(env: Env, warehouse_id: Symbol) -> Result<(), Error> {
        admin::add_warehouse(&env, warehouse_id)
    }

    pub fn remove_warehouse(env: Env, warehouse_id: Symbol) -> Result<(), Error> {
        admin::remove_warehouse(&env, warehouse_id)
    }

    pub fn add_custodian(env: Env, warehouse_id: Symbol, custodian: Address) -> Result<(), Error> {
        admin::add_custodian(&env, warehouse_id, custodian)
    }

    pub fn remove_custodian(
        env: Env,
        warehouse_id: Symbol,
        custodian: Address,
    ) -> Result<(), Error> {
        admin::remove_custodian(&env, warehouse_id, custodian)
    }

    pub fn add_verifier(env: Env, verifier: Address) -> Result<(), Error> {
        admin::add_verifier(&env, verifier)
    }

    pub fn remove_verifier(env: Env, verifier: Address) -> Result<(), Error> {
        admin::remove_verifier(&env, verifier)
    }

    /// Recovery operators co-sign `complete_recovery`.
    pub fn add_recovery_operator(env: Env, operator: Address) -> Result<(), Error> {
        admin::add_recovery_operator(&env, operator)
    }

    pub fn remove_recovery_operator(env: Env, operator: Address) -> Result<(), Error> {
        admin::remove_recovery_operator(&env, operator)
    }
}

impl ReceiptContract {
    /// Receipt ids are only ever written alongside their receipt, so a
    /// missing entry means an archived one; skipping keeps a page readable
    /// rather than failing the whole query.
    fn collect(env: &Env, ids: Vec<u64>) -> Vec<Receipt> {
        let mut out = Vec::new(env);
        for id in ids.iter() {
            if let Some(receipt) = storage::get_receipt(env, id) {
                out.push_back(receipt);
            }
        }
        out
    }
}
