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
//! **Every event is self-contained**: it carries everything the indexer
//! needs to update its projection — owner, warehouse, weights, loan
//! reference, timestamps — so it never has to read the receipt back.
//! Lending events carry `receipt_id` and `lender` as topics. A partial exit,
//! lock or unlock is preceded in the same transaction by a `Split` event
//! linking the new ids to their parent.

use soroban_sdk::{contractevent, Address, Env, String, Symbol};

use crate::types::{Status, WeightSource};

// --- receipt lifecycle -----------------------------------------------------

#[contractevent]
pub struct Minted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub deposit_id: String,
    pub owner: Address,
    pub commodity: Symbol,
    pub grade: Symbol,
    pub weight_kg: u64,
    pub weight_source: WeightSource,
    pub bag_count: u32,
    pub moisture_pct: u32,
    pub custodian: Address,
    pub verifier: Address,
    pub deposit_date: u64,
}

/// Emitted once per split, carrying both children. Weights and bag counts
/// each sum to the parent's; both children inherit `owner`, `warehouse_id`
/// and `status` from it, and the caller's own event then says what became of
/// the taken child.
#[contractevent]
pub struct Split {
    #[topic]
    pub parent_id: u64,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub status: Status,
    pub taken_id: u64,
    pub taken_weight_kg: u64,
    pub taken_bag_count: u32,
    pub remainder_id: u64,
    pub remainder_weight_kg: u64,
    pub remainder_bag_count: u32,
}

#[contractevent]
pub struct Exited {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub owner: Address,
    pub custodian: Address,
    /// Book weight the receipt was denominated in.
    pub weight_kg: u64,
    pub bag_count: u32,
    /// What it actually weighed leaving the warehouse.
    pub exit_weight_kg: u64,
    pub exit_date: u64,
}

#[contractevent]
pub struct Revoked {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub owner: Address,
    pub weight_kg: u64,
    pub reason: String,
    pub revoked_at: u64,
}

#[contractevent]
pub struct TtlExtended {
    #[topic]
    pub receipt_id: u64,
}

// --- lending ---------------------------------------------------------------

#[contractevent]
pub struct Locked {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub kg: u64,
    pub loan_ref: String,
    pub due_date: u64,
    pub grace_period: u64,
    pub locked_at: u64,
}

/// `receipt_id` is the released receipt. On a partial unlock
/// `remaining_receipt_id` is the portion still `Locked` / in `Recovery`,
/// whose weight is `remainder_weight_kg` on the preceding `Split`.
#[contractevent]
pub struct Unlocked {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub loan_ref: String,
    pub kg: u64,
    pub partial: bool,
    pub remaining_receipt_id: Option<u64>,
    pub from_recovery: bool,
    pub unlocked_at: u64,
}

/// The farmer is notified of an extension (design doc §5.4), so `owner` is
/// carried.
#[contractevent]
pub struct DueExtended {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub loan_ref: String,
    pub kg: u64,
    pub old_due_date: u64,
    pub new_due_date: u64,
    pub grace_period: u64,
    pub extended_at: u64,
}

/// `warehouse_id` is a topic so the custodian's notification (design doc
/// §5.5.2) can be routed by filter alone.
#[contractevent]
pub struct RecoveryStarted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub lender: Address,
    #[topic]
    pub warehouse_id: Symbol,
    pub owner: Address,
    pub loan_ref: String,
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
    pub sale_id: u64,
    pub operator: Address,
    pub previous_owner: Address,
    pub buyer: Address,
    pub warehouse_id: Symbol,
    pub loan_ref: String,
    pub kg: u64,
    pub sale_ref: String,
    pub started_at: u64,
    pub completed_at: u64,
}

#[contractevent]
pub struct SettlementRecorded {
    #[topic]
    pub sale_id: u64,
    #[topic]
    pub lender: Address,
    pub receipt_id: u64,
    pub operator: Address,
    pub previous_owner: Address,
    pub buyer: Address,
    pub warehouse_id: Symbol,
    pub loan_ref: String,
    pub kg: u64,
    pub sale_ref: String,
    pub settlement_ref: String,
    pub settled_at: u64,
}

// --- registries ------------------------------------------------------------

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

// Receipt and lending events are published as named struct literals at the
// call site: their payloads are wide enough that a positional helper would be
// easy to misorder. Registry events are narrow and keep helpers.

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
