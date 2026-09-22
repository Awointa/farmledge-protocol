use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,

    DuplicateDepositId = 10,
    ReceiptNotFound = 11,
    InvalidStatus = 12,
    InvalidWeight = 13,
    OnlyScaleEligibleForLock = 14,
    DueDateNotLater = 15,
    RecoveryNotYetDue = 16,
    RecoveryAlreadySettled = 17,

    CustodianNotRegistered = 20,
    VerifierNotRegistered = 21,
    RecoveryOperatorNotRegistered = 22,
    AlreadyRegistered = 23,
    NotRegistered = 24,
}
