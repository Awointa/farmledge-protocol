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
//! linking the new receipt to its parent.
//!
//! **`Split` events are the only on-chain record of a receipt's earlier
//! weights**, because a split reduces the parent in place. The indexer must
//! archive them; see `docs/archival.md`.

use soroban_sdk::{contractevent, Address, BytesN, Env, String, Symbol};

use crate::types::{Status, WeightSource};

// --- receipt lifecycle -----------------------------------------------------

/// A receipt minted from an anchored balance leaf. Every field from
/// `deposit_no` to `deposit_date` is the leaf's (`custodian` and `verifier`
/// attested the intake); `minted_by_custodian` and `minted_by_verifier`
/// signed the mint. `anchor_day` and `leaf_hash` name the leaf, so anyone can
/// check the receipt against the anchored books.
#[contractevent]
pub struct Minted {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub deposit_no: u64,
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
    pub minted_by_custodian: Address,
    pub minted_by_verifier: Address,
    pub anchor_day: u32,
    pub leaf_hash: BytesN<32>,
    pub minted_at: u64,
}

/// Emitted once per split. The parent keeps its id and becomes the
/// remainder, so `remainder_id == parent_id`. `taken_id` is the new receipt.
/// `parent_weight_kg` is the parent's weight before the split, and the two
/// weights and bag counts after it each sum to the parent's before. The new
/// receipt inherits `owner`, `warehouse_id`, `status` and the pledge
/// (`lender`, set when the parent was `Locked` or in `Recovery`), and the
/// caller's own event then says what became of it.
#[contractevent]
pub struct Split {
    #[topic]
    pub parent_id: u64,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub status: Status,
    pub lender: Option<Address>,
    pub parent_weight_kg: u64,
    pub taken_id: u64,
    pub taken_weight_kg: u64,
    pub taken_bag_count: u32,
    pub remainder_id: u64,
    pub remainder_weight_kg: u64,
    pub remainder_bag_count: u32,
    pub split_at: u64,
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

/// Emitted for every receipt `extend_ttl` / `extend_ttl_batch` keeps alive.
/// An `Exited` or `Revoked` receipt is skipped and emits nothing. The host
/// only actually extends an entry whose remaining TTL is under the
/// threshold, so this records the request, not a rent payment.
#[contractevent]
pub struct TtlExtended {
    #[topic]
    pub receipt_id: u64,
    #[topic]
    pub warehouse_id: Symbol,
    pub owner: Address,
    pub status: Status,
    pub weight_kg: u64,
    pub extended_at: u64,
}

// --- anchoring -------------------------------------------------------------

/// One day of a warehouse's books committed. `head` is
/// `sha256(prev_head || day || leaves || root)`, with `day` and `leaves` as
/// 4-byte big-endian and `prev_head` 32 zero bytes before the first anchor.
/// **The only lasting record of `root`**: the stored day root expires after
/// 30 days. The indexer must archive these.
#[contractevent]
pub struct Anchored {
    #[topic]
    pub warehouse_id: Symbol,
    pub day: u32,
    pub leaves: u32,
    pub root: BytesN<32>,
    pub prev_head: BytesN<32>,
    pub head: BytesN<32>,
    pub custodian: Address,
    pub anchored_at: u64,
}

/// A deposit number claimed without a mint, so it can never be minted: a lot
/// already held as a receipt on an earlier contract (audit finding A19).
#[contractevent]
pub struct Preclaimed {
    #[topic]
    pub warehouse_id: Symbol,
    pub deposit_no: u64,
    pub claimed_at: u64,
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
/// `receipt_id` is the new receipt split off and released, and
/// `remaining_receipt_id` is the original receipt, still `Locked` / in
/// `Recovery` with `remainder_weight_kg` from the preceding `Split`.
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
//
// Subject as a topic, timestamp in the data, so a registry change is as
// self-contained as a receipt event.

#[contractevent]
pub struct WarehouseAdded {
    #[topic]
    pub warehouse_id: Symbol,
    pub added_at: u64,
}

#[contractevent]
pub struct WarehouseRemoved {
    #[topic]
    pub warehouse_id: Symbol,
    pub removed_at: u64,
}

#[contractevent]
pub struct CustodianAdded {
    #[topic]
    pub warehouse_id: Symbol,
    pub custodian: Address,
    pub added_at: u64,
}

#[contractevent]
pub struct CustodianRemoved {
    #[topic]
    pub warehouse_id: Symbol,
    pub custodian: Address,
    pub removed_at: u64,
}

#[contractevent]
pub struct VerifierAdded {
    #[topic]
    pub verifier: Address,
    pub added_at: u64,
}

#[contractevent]
pub struct VerifierRemoved {
    #[topic]
    pub verifier: Address,
    pub removed_at: u64,
}

#[contractevent]
pub struct RecoveryOperatorAdded {
    #[topic]
    pub operator: Address,
    pub added_at: u64,
}

#[contractevent]
pub struct RecoveryOperatorRemoved {
    #[topic]
    pub operator: Address,
    pub removed_at: u64,
}

// --- emit helpers ----------------------------------------------------------

// Receipt and lending events are published as named struct literals at the
// call site: their payloads are wide enough that a positional helper would be
// easy to misorder. Registry events are narrow and keep helpers.

pub fn warehouse_added(env: &Env, warehouse_id: &Symbol) {
    WarehouseAdded {
        warehouse_id: warehouse_id.clone(),
        added_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn warehouse_removed(env: &Env, warehouse_id: &Symbol) {
    WarehouseRemoved {
        warehouse_id: warehouse_id.clone(),
        removed_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn custodian_added(env: &Env, warehouse_id: &Symbol, custodian: &Address) {
    CustodianAdded {
        warehouse_id: warehouse_id.clone(),
        custodian: custodian.clone(),
        added_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn custodian_removed(env: &Env, warehouse_id: &Symbol, custodian: &Address) {
    CustodianRemoved {
        warehouse_id: warehouse_id.clone(),
        custodian: custodian.clone(),
        removed_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn verifier_added(env: &Env, verifier: &Address) {
    VerifierAdded {
        verifier: verifier.clone(),
        added_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn verifier_removed(env: &Env, verifier: &Address) {
    VerifierRemoved {
        verifier: verifier.clone(),
        removed_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn recovery_operator_added(env: &Env, operator: &Address) {
    RecoveryOperatorAdded {
        operator: operator.clone(),
        added_at: env.ledger().timestamp(),
    }
    .publish(env);
}

pub fn recovery_operator_removed(env: &Env, operator: &Address) {
    RecoveryOperatorRemoved {
        operator: operator.clone(),
        removed_at: env.ledger().timestamp(),
    }
    .publish(env);
}
