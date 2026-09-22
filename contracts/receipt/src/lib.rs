#![no_std]

mod admin;
mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol};

pub use errors::Error;
pub use types::{Lock, Receipt, Recovery, Status, WeightSource};

#[contract]
pub struct ReceiptContract;

#[contractimpl]
impl ReceiptContract {
    /// One-time setup. `admin` is expected to be a Stellar multisig account
    /// (2-of-3) per design doc §6 — the contract only checks `require_auth`
    /// on it and never implements multisig logic itself.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if admin::get_admin(&env).is_some() {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        admin::set_admin(&env, &admin);
        storage::bump_instance(&env);
        Ok(())
    }

    /// Mints a receipt. Requires custodian AND verifier auth (design doc
    /// §1.2, §4.2); rejects a repeated `deposit_id` or an unregistered
    /// custodian/verifier. Fully implemented — this is the proven path.
    #[allow(clippy::too_many_arguments)]
    pub fn mint(
        env: Env,
        custodian: Address,
        verifier: Address,
        deposit_id: String,
        commodity: Symbol,
        grade: Symbol,
        weight_kg: u64,
        weight_source: WeightSource,
        bag_count: u32,
        moisture_pct: u32,
        warehouse_id: Symbol,
        owner: Address,
    ) -> Result<u64, Error> {
        custodian.require_auth();
        verifier.require_auth();

        if !admin::is_custodian(&env, &warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        if !admin::is_verifier(&env, &verifier) {
            return Err(Error::VerifierNotRegistered);
        }
        if weight_kg == 0 {
            return Err(Error::InvalidWeight);
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
            warehouse_id,
            custodian,
            verifier,
            owner: owner.clone(),
            status: Status::Active,
            deposit_date: env.ledger().timestamp(),
            exit_date: None,
            exit_weight_kg: None,
        };

        storage::set_receipt(&env, &receipt);
        storage::set_deposit_index(&env, &deposit_id, receipt_id);
        storage::bump_instance(&env);
        events::minted(&env, receipt_id, &deposit_id, &owner);

        Ok(receipt_id)
    }

    /// Read-only lookup used by the public verification page and lender
    /// portal (design doc §2.1, §7.1) to read receipt state directly from
    /// the chain rather than Farmledge's database. Fully implemented.
    pub fn get_receipt(env: Env, receipt_id: u64) -> Result<Receipt, Error> {
        storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)
    }

    /// Requires lender AND owner auth. Splits atomically if `kg` is less
    /// than the receipt's weight; only SCALE-weighed, ACTIVE receipts can
    /// be locked (design doc §4.2, §4.3). Body TODO — auth checks are real.
    pub fn lock(
        env: Env,
        receipt_id: u64,
        kg: u64,
        lender: Address,
        loan_ref: String,
        due_date: u64,
        grace_period: u64,
    ) -> Result<u64, Error> {
        lender.require_auth();
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;
        receipt.owner.require_auth();

        let _ = (kg, loan_ref, due_date, grace_period);
        todo!(
            "reject if weight_source != Scale or status != Active; split if kg < weight_kg; \
             set status Locked, store Lock, emit events::locked (+ events::split if partial)"
        )
    }

    /// Lender only. Releases all or part of the locked quantity; a partial
    /// release splits the receipt. Also allowed during RECOVERY if the
    /// farmer repays before the sale completes (design doc §4.2, §5.5).
    pub fn unlock(env: Env, receipt_id: u64, kg: u64) -> Result<(), Error> {
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }
        let lock = storage::get_lock(&env, receipt_id).ok_or(Error::InvalidStatus)?;
        lock.lender.require_auth();

        let _ = kg;
        todo!(
            "reject if status not in [Locked, Recovery]; split if partial; \
             return released portion to Active; clear lock/recovery when fully released"
        )
    }

    /// Lender only. Moves the due date later; cannot move it earlier
    /// (design doc §4.2, §5.4).
    pub fn extend_due(env: Env, receipt_id: u64, new_due: u64) -> Result<(), Error> {
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }
        let lock = storage::get_lock(&env, receipt_id).ok_or(Error::InvalidStatus)?;
        lock.lender.require_auth();

        let _ = new_due;
        todo!("reject if new_due <= lock.due_date; update lock.due_date; emit events::due_extended")
    }

    /// Lender only, after due date plus grace period. Sets status to
    /// RECOVERY; nobody else can move it (design doc §1.2, §4.2, §5.5).
    pub fn start_recovery(env: Env, receipt_id: u64) -> Result<(), Error> {
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }
        let lock = storage::get_lock(&env, receipt_id).ok_or(Error::InvalidStatus)?;
        lock.lender.require_auth();

        todo!(
            "reject if env.ledger().timestamp() < lock.due_date + lock.grace_period \
             (-> Error::RecoveryNotYetDue); set status Recovery; store Recovery::start_date; \
             emit events::recovery_started"
        )
    }

    /// Recovery operator AND lender. Called only after the escrow bank
    /// confirms payment; transfers the receipt to the buyer (design doc
    /// §1.2, §4.2, §5.5). `operator` is explicit because Soroban has no
    /// implicit caller identity to check against a role registry.
    pub fn complete_recovery(
        env: Env,
        receipt_id: u64,
        operator: Address,
        buyer: Address,
        sale_ref: String,
        settlement_ref: String,
    ) -> Result<(), Error> {
        if !admin::is_recovery_operator(&env, &operator) {
            return Err(Error::RecoveryOperatorNotRegistered);
        }
        operator.require_auth();
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }
        let lock = storage::get_lock(&env, receipt_id).ok_or(Error::InvalidStatus)?;
        lock.lender.require_auth();

        let _ = (buyer, sale_ref, settlement_ref);
        todo!(
            "reject if status != Recovery; set recovery.buyer/sale_ref/settlement_ref; \
             transfer owner to buyer; emit events::recovery_completed"
        )
    }

    /// Custodian AND owner. Splits if partial, burns the exited portion and
    /// records actual exit weight. Rejected if LOCKED or in RECOVERY
    /// (design doc §4.2, §4.3, §5.6). `custodian` is explicit — exit can be
    /// performed by any custodian registered for the receipt's warehouse,
    /// not only the one who signed the original mint.
    pub fn exit(
        env: Env,
        receipt_id: u64,
        custodian: Address,
        kg: u64,
        exit_weight: u64,
    ) -> Result<u64, Error> {
        let receipt = storage::get_receipt(&env, receipt_id).ok_or(Error::ReceiptNotFound)?;
        if !admin::is_custodian(&env, &receipt.warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        custodian.require_auth();
        receipt.owner.require_auth();

        if matches!(receipt.status, Status::Locked | Status::Recovery) {
            return Err(Error::InvalidStatus);
        }

        let _ = (kg, exit_weight);
        todo!(
            "split if kg < weight_kg; burn exited portion; set status Exited on the \
             fully-exited receipt; record exit_date/exit_weight_kg; emit events::exited"
        )
    }

    /// Admin multisig only. Used for proven fraudulent mints (design doc
    /// §1.2, §9).
    pub fn revoke(env: Env, receipt_id: u64, reason: String) -> Result<(), Error> {
        admin::require_admin(&env)?;
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }

        let _ = reason;
        todo!("set status Revoked regardless of current status; emit events::revoked")
    }

    /// Callable by anyone; keeps active receipts from being archived
    /// (design doc §1.2, §7.2). No auth required by design.
    pub fn extend_ttl(env: Env, receipt_id: u64) -> Result<(), Error> {
        if !storage::has_receipt(&env, receipt_id) {
            return Err(Error::ReceiptNotFound);
        }

        todo!("storage::bump_receipt(&env, receipt_id); emit events::ttl_extended")
    }

    // --- Admin registry functions (design doc §1.2) -----------------------

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

    pub fn add_recovery_operator(env: Env, operator: Address) -> Result<(), Error> {
        admin::add_recovery_operator(&env, operator)
    }

    pub fn remove_recovery_operator(env: Env, operator: Address) -> Result<(), Error> {
        admin::remove_recovery_operator(&env, operator)
    }
}
