use soroban_sdk::{contracttype, Address, BytesN, String, Symbol, Vec};

/// Layout version of every `Receipt` and `Sale` this build writes.
///
/// The contract is immutable (see `docs/upgrade.md`), so a layout change
/// ships as a new deployment. Stamping each stored record with the layout it
/// was written in lets a migration tool, or a future contract that reads
/// this one, decode old records without guessing. Bump it whenever a stored
/// type's fields change.
///
/// - 1: named-field maps (the first Testnet deployment).
/// - 2: the positional layout in `layout.rs`; splits divide in place.
/// - 3: receipts are minted on demand from an anchored balance leaf and
///   carry its `deposit_no`; see `anchor.rs`.
pub const DATA_VERSION: u32 = 3;

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
/// ```
///
/// A partial exit, lock or unlock first splits the receipt *in place*: the
/// receipt keeps its status and the remaining weight, and the portion acted
/// on becomes a new receipt (see `split::split_receipt`). Splitting is not
/// a status.
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
/// recovery can overwrite it.
///
/// A completed recovery points at its canonical [`Sale`] by `sale_id` rather
/// than carrying the settlement itself: the buyer may split the receipt
/// before the bank settles, and every child's copy of this record must
/// resolve to the same, later-completed settlement.
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
    /// Set only on `Completed`.
    pub sale_id: Option<u64>,
    /// Set only on `Completed`.
    pub buyer: Option<Address>,
    /// Escrow deposit reference proving the buyer paid. Set only on
    /// `Completed`.
    pub sale_ref: Option<String>,
}

/// The canonical record of one recovery sale, stored once under its
/// `sale_id` no matter how the sold receipt is later divided.
///
/// Everything except the settlement is fixed at `complete_recovery`.
/// `settlement_ref` — the bank's payout to the lender and any surplus to the
/// farmer — arrives later through `record_settlement` and is write-once.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sale {
    /// Layout version this record was written in; see [`DATA_VERSION`].
    pub data_version: u32,
    pub sale_id: u64,
    /// The receipt that was sold, as it was at the moment of sale.
    pub receipt_id: u64,
    pub warehouse_id: Symbol,
    pub weight_kg: u64,
    pub lender: Address,
    pub loan_ref: String,
    pub operator: Address,
    pub previous_owner: Address,
    pub buyer: Address,
    pub sale_ref: String,
    pub started_at: u64,
    pub completed_at: u64,
    pub settlement_ref: Option<String>,
    pub settled_at: Option<u64>,
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
/// quantity the conservation invariant applies to: across every receipt of
/// one deposit it always sums to the minted weight. It is **not fixed for
/// life**. A partial exit, lock or unlock moves part of it onto a new
/// receipt, and the `Split` event records each such division. `exit_weight_kg` is what
/// the grain actually weighed on the way out, which differs by shrinkage —
/// recording both is the point of the exit flow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    /// Layout version this record was written in; see [`DATA_VERSION`].
    /// Every write re-stamps it, so it is always the current one.
    pub data_version: u32,
    /// Contract-assigned, auto-incrementing. Distinct from `deposit_id`.
    pub receipt_id: u64,
    /// The lot's human intake reference, from the leaf it was minted from.
    /// A receipt split off another inherits its parent's, since they are the
    /// same physical deposit.
    pub deposit_id: String,
    /// The lot's per-warehouse number: its identity in the anchored books,
    /// and the key of the duplicate guard. Inherited by every split.
    pub deposit_no: u64,
    /// The receipt this one was split off, `None` on a minted receipt. The
    /// parent itself lives on with the remaining weight.
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
    /// Every resolved recovery, oldest first. Append-only; a receipt split
    /// off another inherits its parent's.
    pub recovery_history: Vec<Recovery>,
    pub deposit_date: u64,
    pub exit_date: Option<u64>,
    /// Actual weighed quantity at exit; may differ from `weight_kg`.
    pub exit_weight_kg: Option<u64>,
}

/// Names one paged index, for `extend_index_ttl`. Each variant matches a
/// paged read: `get_receipts_by_owner`, `get_receipts_by_warehouse`,
/// `get_receipts_by_lender` and `get_sales_by_lender`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Index {
    Owner(Address),
    Warehouse(Symbol),
    Lender(Address),
    LenderSales(Address),
}

/// One leaf of a warehouse's daily tree, as the backend builds it and the
/// farmer holds it. Hashed as `sha256(0x00 || XDR(leaf))`; see
/// `docs/storage-strategy.md` ("Leaf format") and `docs/events.md`
/// ("Farmer proofs").
///
/// **Its layout is part of the proof format**: every proof ever handed to a
/// farmer is checked against it. Never change a field; a new layout is a new
/// `version`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leaf {
    /// `LEAF_VERSION`.
    pub version: u32,
    /// 0 deposit, 1 dispatch, 2 end-of-day balance. Only a balance mints.
    pub kind: u32,
    pub warehouse_id: Symbol,
    /// UTC days since 1970-01-01: the day whose tree this leaf is in.
    pub day: u32,
    /// The leaf's position in that tree. A position, never an identity.
    pub index: u32,
    /// The lot's identity: assigned once at intake, never reused.
    pub deposit_no: u64,
    pub deposit_id: String,
    /// The receipt that governs the lot, or 0 if none.
    pub receipt_id: u64,
    /// The `data_version` of the contract `receipt_id` belongs to, or 0.
    pub receipt_version: u32,
    pub owner: Address,
    pub commodity: Symbol,
    pub grade: Symbol,
    /// Deposited, dispatched or remaining, by `kind`.
    pub kg: u64,
    pub weight_source: WeightSource,
    pub bags: u32,
    /// Basis points of a percent, as `Receipt::moisture_pct`.
    pub moisture_bps: u32,
    /// Who attested the intake.
    pub custodian: Address,
    /// Who countersigned the intake.
    pub verifier: Address,
    /// When the lot was taken in (Unix seconds). The same on every leaf of
    /// the lot.
    pub deposited_at: u64,
    /// When this leaf's event happened; the day's cutoff for a balance.
    pub at: u64,
    /// Random, per leaf, so a leaf hash reveals nothing to anyone without it.
    pub salt: BytesN<32>,
}

/// A warehouse's latest anchor: the last day anchored, and the hash chain's
/// head after it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorHead {
    pub day: u32,
    pub head: BytesN<32>,
}
