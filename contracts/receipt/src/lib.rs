#![no_std]
//! Farmledge digital warehouse receipts.
//!
//! One contract for all commodities — commodity is a field on the receipt,
//! not a separate deployment.
//!
//! This build covers the deposit-to-exit lifecycle: `mint`, `exit` (full and
//! partial), `revoke` and `extend_ttl`. The lending states (`Locked`,
//! `Recovery`) and their `Lock` / `Recovery` payloads are defined and stored
//! on every receipt so that `lock`, `unlock`, `extend_due`, `start_recovery`,
//! `complete_recovery` and `transfer` can be added without a migration.
//!
//! Soroban has no implicit caller identity. Every address whose signature an
//! operation needs is an explicit parameter that gets `require_auth()`d;
//! privileged addresses (the admin) are loaded from storage instead, because
//! authorising a caller-supplied address proves only that the caller controls
//! it, not that it is allowed to do anything.

mod admin;
mod errors;
mod events;
mod split;
mod storage;
mod types;

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol, Vec};

pub use errors::Error;
pub use types::{Lock, MaybeLock, MaybeRecovery, Receipt, Recovery, Status, WeightSource};

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
            deposit_date: env.ledger().timestamp(),
            exit_date: None,
            exit_weight_kg: None,
        };

        storage::set_receipt(&env, &receipt);
        storage::set_deposit_index(&env, &deposit_id, receipt_id);
        storage::index_owner(&env, &farmer, receipt_id);
        storage::index_warehouse(&env, &warehouse_id, receipt_id);
        storage::bump_instance(&env);

        events::minted(
            &env,
            receipt_id,
            &warehouse_id,
            &deposit_id,
            &farmer,
            weight_kg,
        );
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

        events::exited(
            &env,
            exiting.receipt_id,
            &custodian,
            exiting.weight_kg,
            exit_weight_kg,
        );
        Ok(exiting.receipt_id)
    }

    /// Voids a receipt. Admin only, and only from `Active` — a receipt whose
    /// grain has already left, or that is already void, is not revocable.
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

        events::revoked(&env, receipt_id, &reason);
        Ok(())
    }

    /// Pushes a receipt's storage rent out. Callable by **anyone**, with no
    /// authorisation, so a keep-alive job can run unprivileged — and so can
    /// anyone else with an interest in the receipt surviving.
    ///
    /// Also bumps the receipt's `deposit_id` guard entry, keeping the
    /// duplicate-mint protection alive alongside the receipt it protects.
    pub fn extend_ttl(env: Env, receipt_id: u64) -> Result<(), Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;

        storage::bump_receipt(&env, receipt_id);
        storage::bump_deposit_index(&env, &receipt.deposit_id);

        events::ttl_extended(&env, receipt_id);
        Ok(())
    }

    // --- reads -------------------------------------------------------------

    pub fn get_receipt(env: Env, receipt_id: u64) -> Result<Receipt, Error> {
        storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)
    }

    /// One page of an owner's receipts, newest last, 64 per page.
    ///
    /// Paged because an unpaged read would blow the network's per-transaction
    /// ledger-read limit (200 entries) once a farmer has a few hundred
    /// receipts. Includes `Split`, `Exited` and `Revoked` records — they are
    /// the audit trail; filter on `status` for live holdings.
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

    /// Stored now, consumed by `complete_recovery` next session.
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
