use soroban_sdk::{contracttype, Address, String, Symbol};

/// How a receipt's weight was determined. Only `Scale` receipts are eligible
/// for collateral locks (bag-derived weights vary with moisture and filling).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WeightSource {
    BagEstimate,
    Scale,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Active,
    Locked,
    Recovery,
    Exited,
    Revoked,
}

/// Collateral pledge. Interest rates and fees are never stored on-chain —
/// lenders keep loan terms in their own systems (design doc §3.4, §4.1).
/// Stored separately from `Receipt` (storage.rs `DataKey::Lock`), not as an
/// `Option<Lock>` field on it: soroban-sdk's spec-shaking codegen can't
/// derive an infallible `ScVal` conversion for `Option<CustomStruct>` under
/// `cargo test`, so nested custom-struct Options are avoided entirely.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lock {
    pub lender: Address,
    pub loan_ref: String,
    pub due_date: u64,
    pub grace_period: u64,
}

/// Default recovery via sale through bank escrow (design doc §5.5).
/// `buyer`/`sale_ref`/`settlement_ref` are only populated on completion.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub start_date: u64,
    pub buyer: Option<Address>,
    pub sale_ref: Option<String>,
    pub settlement_ref: Option<String>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub receipt_id: u64,
    pub deposit_id: String,
    pub parent_id: Option<u64>,
    pub commodity: Symbol,
    pub grade: Symbol,
    pub weight_kg: u64,
    pub weight_source: WeightSource,
    pub bag_count: u32,
    /// Moisture percentage in basis points of a percent (e.g. 1250 = 12.50%).
    /// Soroban has no floating point, so fixed-point integers are used.
    pub moisture_pct: u32,
    pub warehouse_id: Symbol,
    pub custodian: Address,
    pub verifier: Address,
    pub owner: Address,
    pub status: Status,
    pub deposit_date: u64,
    pub exit_date: Option<u64>,
    pub exit_weight_kg: Option<u64>,
}
