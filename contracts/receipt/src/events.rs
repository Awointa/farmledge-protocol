//! One typed event per state change, using `#[contractevent]` (the SDK
//! deprecated raw `env.events().publish()` in favour of it).
//!
//! Field order matters: `#[topic]` fields become topics after the event name,
//! the rest form the data payload. Indexers filter on topics, so receipt ids
//! and registry subjects are topics.
//!
//! Events are how `farmledge-app` mirrors chain state, but they are
//! **ephemeral** — RPC providers retain roughly 7 days. Anything needed
//! long-term has to be derivable from receipts in storage, and it is: every
//! event here is a projection of a `Receipt` that remains readable.
//!
//! Every lending event carries `receipt_id` and `lender` as topics, plus the
//! receipt's `kg` and the timestamps of the transition, so the backend
//! indexer can follow a loan without reading storage. A partial lock or
//! unlock is preceded in the same transaction by a `Split` event linking the
//! new ids to their parent.

use soroban_sdk::{contractevent, Address, Env, String, Symbol};

#[contractevent]
pub struct Minted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub deposit_id: String,
    pub owner: Address,
    pub weight_kg: u64,
}

/// Emitted once per split, carrying both children. The two child weights
/// always sum to the parent's.
#[contractevent]
pub struct Split {
    #[topic]
    pub parent_id: u64,
    pub taken_id: u64,
    pub taken_weight_kg: u64,
    pub remainder_id: u64,
    pub remainder_weight_kg: u64,
}

#[contractevent]
pub struct Exited {
    #[topic]
    pub receipt_id: u64,
    pub custodian: Address,
    /// Book weight the receipt was denominated in.
    pub weight_kg: u64,
    /// What it actually weighed leaving the warehouse.
    pub exit_weight_kg: u64,
}

#[contractevent]
pub struct Revoked {
    #[topic]
    pub receipt_id: u64,
    pub reason: String,
}

#[contractevent]
pub struct TtlExtended {
    #[topic]
    pub receipt_id: u64,
}

#[contractevent]
pub struct Locked {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub kg: u64,
    pub loan_ref: String,
    pub due_date: u64,
    pub grace_period: u64,
    pub locked_at: u64,
}

/// `receipt_id` is the released receipt. On a partial unlock
/// `remaining_receipt_id` is the portion still `Locked` / in `Recovery`.
#[contractevent]
pub struct Unlocked {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub kg: u64,
    pub partial: bool,
    pub remaining_receipt_id: Option<u64>,
    pub from_recovery: bool,
    pub unlocked_at: u64,
}

#[contractevent]
pub struct DueExtended {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub kg: u64,
    pub old_due_date: u64,
    pub new_due_date: u64,
    pub extended_at: u64,
}

#[contractevent]
pub struct RecoveryStarted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub kg: u64,
    pub due_date: u64,
    pub grace_period: u64,
    pub started_at: u64,
}

#[contractevent]
pub struct RecoveryCompleted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub operator: Address,
    pub previous_owner: Address,
    pub buyer: Address,
    pub kg: u64,
    pub sale_ref: String,
    pub settlement_ref: String,
    pub started_at: u64,
    pub completed_at: u64,
}

#[contractevent]
pub struct WarehouseAdded {
    #[topic]
    pub warehouse_id: Symbol,
}

#[contractevent]
pub struct WarehouseRemoved {
    #[topic]
    pub warehouse_id: Symbol,
}

#[contractevent]
pub struct CustodianAdded {
    #[topic]
    pub warehouse_id: Symbol,
    pub custodian: Address,
}

#[contractevent]
pub struct CustodianRemoved {
    #[topic]
    pub warehouse_id: Symbol,
    pub custodian: Address,
}

#[contractevent]
pub struct VerifierAdded {
    #[topic]
    pub verifier: Address,
}

#[contractevent]
pub struct VerifierRemoved {
    #[topic]
    pub verifier: Address,
}

#[contractevent]
pub struct RecoveryOperatorAdded {
    #[topic]
    pub operator: Address,
}

#[contractevent]
pub struct RecoveryOperatorRemoved {
    #[topic]
    pub operator: Address,
}

// --- emit helpers ----------------------------------------------------------

pub fn minted(
    env: &Env,
    receipt_id: u64,
    warehouse_id: &Symbol,
    deposit_id: &String,
    owner: &Address,
    weight_kg: u64,
) {
    Minted {
        receipt_id,
        warehouse_id: warehouse_id.clone(),
        deposit_id: deposit_id.clone(),
        owner: owner.clone(),
        weight_kg,
    }
    .publish(env);
}

pub fn split(
    env: &Env,
    parent_id: u64,
    taken_id: u64,
    taken_weight_kg: u64,
    remainder_id: u64,
    remainder_weight_kg: u64,
) {
    Split {
        parent_id,
        taken_id,
        taken_weight_kg,
        remainder_id,
        remainder_weight_kg,
    }
    .publish(env);
}

pub fn exited(
    env: &Env,
    receipt_id: u64,
    custodian: &Address,
    weight_kg: u64,
    exit_weight_kg: u64,
) {
    Exited {
        receipt_id,
        custodian: custodian.clone(),
        weight_kg,
        exit_weight_kg,
    }
    .publish(env);
}

pub fn revoked(env: &Env, receipt_id: u64, reason: &String) {
    Revoked {
        receipt_id,
        reason: reason.clone(),
    }
    .publish(env);
}

pub fn ttl_extended(env: &Env, receipt_id: u64) {
    TtlExtended { receipt_id }.publish(env);
}

// Lending events are built by the caller (`lending.rs`) and published
// directly: their payloads are wide enough that a positional helper would be
// easier to misorder than the named struct literal.

pub fn warehouse_added(env: &Env, warehouse_id: &Symbol) {
    WarehouseAdded {
        warehouse_id: warehouse_id.clone(),
    }
    .publish(env);
}

pub fn warehouse_removed(env: &Env, warehouse_id: &Symbol) {
    WarehouseRemoved {
        warehouse_id: warehouse_id.clone(),
    }
    .publish(env);
}

pub fn custodian_added(env: &Env, warehouse_id: &Symbol, custodian: &Address) {
    CustodianAdded {
        warehouse_id: warehouse_id.clone(),
        custodian: custodian.clone(),
    }
    .publish(env);
}

pub fn custodian_removed(env: &Env, warehouse_id: &Symbol, custodian: &Address) {
    CustodianRemoved {
        warehouse_id: warehouse_id.clone(),
        custodian: custodian.clone(),
    }
    .publish(env);
}

pub fn verifier_added(env: &Env, verifier: &Address) {
    VerifierAdded {
        verifier: verifier.clone(),
    }
    .publish(env);
}

pub fn verifier_removed(env: &Env, verifier: &Address) {
    VerifierRemoved {
        verifier: verifier.clone(),
    }
    .publish(env);
}

pub fn recovery_operator_added(env: &Env, operator: &Address) {
    RecoveryOperatorAdded {
        operator: operator.clone(),
    }
    .publish(env);
}

pub fn recovery_operator_removed(env: &Env, operator: &Address) {
    RecoveryOperatorRemoved {
        operator: operator.clone(),
    }
    .publish(env);
}
