use soroban_sdk::contracterror;

/// Every failure path has its own variant. Contract code never uses a bare
/// `panic!("msg")` — those surface as opaque host errors that clients cannot
/// match on. Fallible entry points return `Result<_, Error>`; the few places
/// that must abort use `panic_with_error!`.
///
/// **Error codes are public ABI. Never renumber them.** Codes 30-33 are
/// reserved now, unused, so that the lending functions landing next session
/// do not force a renumbering of everything below them.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // --- lifecycle of the contract itself ---
    NotInitialized = 1,
    AlreadyInitialized = 2,

    // --- receipts ---
    ReceiptNotFound = 10,
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

    // --- registries ---
    WarehouseNotRegistered = 20,
    CustodianNotRegistered = 21,
    VerifierNotRegistered = 22,
    RecoveryOperatorNotRegistered = 23,
    AlreadyRegistered = 24,
    NotRegistered = 25,

    // --- reserved for lock / unlock / extend_due / recovery (next session) ---
    OnlyScaleEligibleForLock = 30,
    DueDateNotLater = 31,
    RecoveryNotYetDue = 32,
    RecoveryAlreadySettled = 33,
}
