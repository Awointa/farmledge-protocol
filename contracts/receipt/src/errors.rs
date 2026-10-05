use soroban_sdk::contracterror;

/// Every failure path has its own variant. Contract code never uses a bare
/// `panic!("msg")` — those surface as opaque host errors that clients cannot
/// match on. Fallible entry points return `Result<_, Error>`; the few places
/// that must abort use `panic_with_error!`.
///
/// **Error codes are public ABI. Never renumber them.** New variants take
/// the next free code.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // --- lifecycle of the contract itself ---
    NotInitialized = 1,
    AlreadyInitialized = 2,

    // --- receipts ---
    ReceiptNotFound = 10,
    /// The lot's `(warehouse_id, deposit_no)` is already claimed, by a mint
    /// or a pre-claim.
    DuplicateDepositId = 11,
    /// The receipt is not in a status this operation accepts.
    InvalidStatus = 12,
    /// `weight_kg` was zero, or `bag_count` was zero for a bag-estimated lot.
    InvalidWeight = 13,
    /// Split quantity was zero, or exceeded the receipt's weight.
    InvalidSplitAmount = 14,
    /// Moisture outside 0..=10000 basis points of a percent.
    InvalidMoisture = 15,
    /// Recorded exit weight was zero.
    InvalidExitWeight = 16,
    /// A stored record holds a code no layout defines. Only reachable if
    /// storage was written by something other than this contract.
    CorruptRecord = 17,

    // --- registries ---
    WarehouseNotRegistered = 20,
    CustodianNotRegistered = 21,
    VerifierNotRegistered = 22,
    RecoveryOperatorNotRegistered = 23,
    AlreadyRegistered = 24,
    NotRegistered = 25,

    // --- lending ---
    /// Only `WeightSource::Scale` receipts can be pledged.
    OnlyScaleEligibleForLock = 30,
    /// `extend_due` must move the due date strictly later.
    DueDateNotLater = 31,
    /// `start_recovery` before `due_date + grace_period` has passed.
    RecoveryNotYetDue = 32,
    /// `record_settlement` on a sale whose settlement is already recorded.
    RecoveryAlreadySettled = 33,
    /// `get_lock` on a receipt that is not `Locked` or `Recovery`.
    NotLocked = 34,
    /// `due_date` not in the future, or `due_date + grace_period` overflows.
    InvalidDueDate = 35,
    /// A receipt cannot be pledged to its own owner.
    LenderIsOwner = 36,
    /// No sale with this `sale_id` — the recovery was never completed.
    SaleNotFound = 37,
    /// `deposit_id`, `loan_ref`, `sale_ref` or `settlement_ref` longer than
    /// `MAX_REF_LEN`.
    RefTooLong = 38,

    // --- operations ---
    /// A batched call named more entries than `MAX_TTL_BATCH`.
    BatchTooLarge = 40,

    // --- anchoring and minting on demand ---
    /// Only a balance leaf (kind 2) can mint: a deposit or dispatch leaf
    /// records a movement, not what the lot holds now.
    WrongLeafKind = 50,
    /// The leaf is not from the warehouse's latest anchored day.
    StaleLeaf = 51,
    /// The proof does not fold to the anchored root, has the wrong length
    /// for that day's tree, or names a position outside it.
    InvalidProof = 52,
    /// The warehouse has never been anchored, or the day's root has expired.
    NoAnchor = 53,
    /// `anchor` for a day at or before the warehouse's last anchored day.
    AnchorOutOfOrder = 54,
    /// `anchor` for a day after the ledger's current UTC day.
    AnchorInFuture = 55,
    /// `anchor` with no leaves, or more than `2^MAX_TREE_DEPTH`.
    InvalidLeafCount = 56,
    /// The leaf says the lot is already governed by a receipt.
    LeafHasReceipt = 57,
    /// A leaf `version` this contract does not know.
    UnsupportedLeafVersion = 58,
}
