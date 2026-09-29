use soroban_sdk::{contracttype, Address, String, Symbol, Vec};

/// How a receipt's weight was determined.
///
/// Phase 1 field testing is bag-count intake (`BagEstimate`); only `Scale`
/// receipts are eligible for collateral locks, because bag-derived weights
/// vary with moisture and how full each bag is. The rule lives in one place:
/// `lending::require_scale_weight`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WeightSource {
    BagEstimate,
    Scale,
}

/// Receipt lifecycle. The only legal transitions:
///
/// ```text
/// Active   -> Locked     lock
/// Active   -> Exited     exit
/// Active   -> Revoked    revoke (admin)
/// Locked   -> Active     unlock (full or partial)
/// Locked   -> Locked     extend_due
/// Locked   -> Recovery   start_recovery
/// Recovery -> Active     unlock (farmer repaid before the sale completed)
/// Recovery -> Active     complete_recovery (owner becomes the buyer)
/// any live -> Split      internal, as part of a partial exit/lock/unlock
/// ```
///
/// A `Locked` or `Recovery` receipt can never be exited, revoked, or split by
/// its owner. To void a pledged receipt the lender unlocks it first.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    /// Grain is in the warehouse, free of any pledge.
    Active,
    /// Pledged as loan collateral. `lock` is `MaybeLock::Some`.
    Locked,
    /// Lender has begun default recovery. `lock` and `recovery` are both set.
    Recovery,
    /// Superseded by a split: this receipt was divided into exactly two
    /// children whose weights sum to it. Retained as an immutable audit
    /// record and never mutated again. See `split::split_receipt`.
    Split,
    /// Grain has left the warehouse. Terminal.
    Exited,
    /// Voided by the admin (e.g. a proven fraudulent mint). Terminal.
    Revoked,
}

/// Collateral pledge. Interest rates and fees are deliberately **never**
/// stored on-chain — lenders keep loan terms in their own systems and
/// reference them through `loan_ref`.
///
/// `due_date` and `grace_period` are ledger-timestamp seconds supplied by the
/// lender. The contract enforces their ordering, not their length.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lock {
    pub lender: Address,
    pub loan_ref: String,
    pub due_date: u64,
    pub grace_period: u64,
}

/// How a recovery ended.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryResolution {
    /// In progress: the receipt is in `Status::Recovery`.
    Open,
    /// The lender unlocked before the sale completed.
    Repaid,
    /// Sold through escrow; ownership passed to `buyer`.
    Completed,
}

/// One default recovery.
///
/// While open it sits in `Receipt::recovery`. When it resolves — `Repaid` by
/// an unlock, `Completed` by a sale — it is moved into
/// `Receipt::recovery_history`, which is append-only, so no later lock or
/// recovery can overwrite it. The sale fields are only set on `Completed`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub lender: Address,
    pub loan_ref: String,
    /// Owner of the receipt when recovery began.
    pub owner: Address,
    pub started_at: u64,
    pub resolution: RecoveryResolution,
    pub resolved_at: Option<u64>,
    pub buyer: Option<Address>,
    pub sale_ref: Option<String>,
    pub settlement_ref: Option<String>,
}

/// Stands in for `Option<Lock>`.
///
/// `Option<CustomStruct>` cannot be a field of a `#[contracttype]` struct on
/// soroban-sdk 28: the `Option<T> -> ScVal` conversion requires an infallible
/// `T: Into<ScVal>`, but `#[contracttype]` only generates the fallible
/// `TryFrom<&T> for ScVal`. Native and wasm builds happen to pass; the
/// `testutils` build used by `cargo test` fails with
/// `the trait bound `ScVal: TryFrom<&Option<Lock>>` is not satisfied`.
///
/// A two-variant enum carries identical semantics and round-trips cleanly
/// through storage and the generated client. `Option<u64>` and friends are
/// unaffected — only `Option` of a custom struct is.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaybeLock {
    None,
    Some(Lock),
}

/// Stands in for `Option<Recovery>` — see [`MaybeLock`].
///
/// The variants differ in size by the width of a `Recovery`, which clippy
/// flags. Boxing the payload is not an option: the layout crosses the
/// contract ABI, and every `Receipt` carries one. It costs in-memory width,
/// not storage — `MaybeRecovery::None` serialises to a bare variant symbol.
#[allow(clippy::large_enum_variant)]
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaybeRecovery {
    None,
    Some(Recovery),
}

/// A digital warehouse receipt for one lot of grain.
///
/// `weight_kg` is the *book* weight the receipt is denominated in and is the
/// quantity the conservation invariant applies to. `exit_weight_kg` is what
/// the grain actually weighed on the way out, which differs by shrinkage —
/// recording both is the point of the exit flow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    /// Contract-assigned, auto-incrementing. Distinct from `deposit_id`.
    pub receipt_id: u64,
    /// Client-generated intake reference. Unique across mints; children of a
    /// split inherit their parent's, since they are the same physical deposit.
    pub deposit_id: String,
    /// Set on both children of a split, `None` on a minted receipt.
    pub parent_id: Option<u64>,
    pub commodity: Symbol,
    pub grade: Symbol,
    pub weight_kg: u64,
    pub weight_source: WeightSource,
    pub bag_count: u32,
    /// Moisture in basis points of a percent (1250 = 12.50%). Soroban has no
    /// floating point, so this is fixed-point.
    pub moisture_pct: u32,
    pub warehouse_id: Symbol,
    /// The custodian who attested the intake.
    pub custodian: Address,
    /// The verifier who countersigned the intake.
    pub verifier: Address,
    pub owner: Address,
    pub status: Status,
    /// Set while `Locked` or `Recovery`.
    pub lock: MaybeLock,
    /// The open recovery; set only while `Recovery`.
    pub recovery: MaybeRecovery,
    /// Every resolved recovery, oldest first. Append-only; split children
    /// inherit their parent's.
    pub recovery_history: Vec<Recovery>,
    pub deposit_date: u64,
    pub exit_date: Option<u64>,
    /// Actual weighed quantity at exit; may differ from `weight_kg`.
    pub exit_weight_kg: Option<u64>,
}
