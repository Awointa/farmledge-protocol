use soroban_sdk::{contractevent, Address, Env, String, Symbol};

/// Typed events, one per state change (design doc §4.1), using the current
/// `#[contractevent]` macro — the SDK deprecated raw `env.events().publish()`
/// in favor of this in soroban-sdk 26+. Only `Minted` is actually emitted in
/// this Phase 1 skeleton; the rest are ready for their TODO function bodies.

#[contractevent]
pub struct Minted {
    #[topic]
    pub receipt_id: u64,
    pub deposit_id: String,
    pub owner: Address,
}

#[contractevent]
pub struct Locked {
    #[topic]
    pub receipt_id: u64,
    pub lender: Address,
    pub kg: u64,
}

#[contractevent]
pub struct Unlocked {
    #[topic]
    pub receipt_id: u64,
    pub kg: u64,
}

#[contractevent]
pub struct Split {
    #[topic]
    pub parent_id: u64,
    pub child_id: u64,
    pub child_weight_kg: u64,
}

#[contractevent]
pub struct DueExtended {
    #[topic]
    pub receipt_id: u64,
    pub new_due: u64,
}

#[contractevent]
pub struct RecoveryStarted {
    #[topic]
    pub receipt_id: u64,
    pub start_date: u64,
}

#[contractevent]
pub struct RecoveryCompleted {
    #[topic]
    pub receipt_id: u64,
    pub buyer: Address,
}

#[contractevent]
pub struct Exited {
    #[topic]
    pub receipt_id: u64,
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

pub fn minted(env: &Env, receipt_id: u64, deposit_id: &String, owner: &Address) {
    Minted {
        receipt_id,
        deposit_id: deposit_id.clone(),
        owner: owner.clone(),
    }
    .publish(env);
}

pub fn locked(env: &Env, receipt_id: u64, lender: &Address, kg: u64) {
    Locked {
        receipt_id,
        lender: lender.clone(),
        kg,
    }
    .publish(env);
}

pub fn unlocked(env: &Env, receipt_id: u64, kg: u64) {
    Unlocked { receipt_id, kg }.publish(env);
}

pub fn split(env: &Env, parent_id: u64, child_id: u64, child_weight_kg: u64) {
    Split {
        parent_id,
        child_id,
        child_weight_kg,
    }
    .publish(env);
}

pub fn due_extended(env: &Env, receipt_id: u64, new_due: u64) {
    DueExtended {
        receipt_id,
        new_due,
    }
    .publish(env);
}

pub fn recovery_started(env: &Env, receipt_id: u64, start_date: u64) {
    RecoveryStarted {
        receipt_id,
        start_date,
    }
    .publish(env);
}

pub fn recovery_completed(env: &Env, receipt_id: u64, buyer: &Address) {
    RecoveryCompleted {
        receipt_id,
        buyer: buyer.clone(),
    }
    .publish(env);
}

pub fn exited(env: &Env, receipt_id: u64, exit_weight_kg: u64) {
    Exited {
        receipt_id,
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

pub fn custodian_added(env: &Env, warehouse_id: Symbol, custodian: &Address) {
    CustodianAdded {
        warehouse_id,
        custodian: custodian.clone(),
    }
    .publish(env);
}

pub fn custodian_removed(env: &Env, warehouse_id: Symbol, custodian: &Address) {
    CustodianRemoved {
        warehouse_id,
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
