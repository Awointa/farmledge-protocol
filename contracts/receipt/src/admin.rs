//! Admin identity and the four registries.
//!
//! `admin` is treated as a single `Address`. In production it will be a
//! Stellar multisig account, which satisfies `require_auth()` at its medium
//! threshold with no contract-side support — so there is deliberately no
//! multisig logic here.

use soroban_sdk::{Address, Env, Symbol};

use crate::errors::Error;
use crate::events;
use crate::storage::{bump, bump_instance, DataKey};

// --- admin -----------------------------------------------------------------

pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Admin)
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

/// Loads the admin from storage and authorises *that* address.
///
/// Never takes a caller-supplied address: `who.require_auth()` on a parameter
/// proves only that the caller controls `who`, not that `who` is privileged.
pub fn require_admin(env: &Env) -> Result<(), Error> {
    let admin = get_admin(env).ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

// --- registry reads --------------------------------------------------------
//
// Registry entries are persistent, so they carry their own TTL and can be
// archived. Reading one during mint/exit bumps it, which means an actively
// used warehouse keeps its own registration (and its custodians') alive
// without anyone running a keep-alive job for it.

pub fn is_warehouse(env: &Env, warehouse_id: &Symbol) -> bool {
    let key = DataKey::Warehouse(warehouse_id.clone());
    let found = env.storage().persistent().has(&key);
    if found {
        bump(env, &key);
    }
    found
}

pub fn is_custodian(env: &Env, warehouse_id: &Symbol, custodian: &Address) -> bool {
    let key = DataKey::Custodian(warehouse_id.clone(), custodian.clone());
    let found = env.storage().persistent().has(&key);
    if found {
        bump(env, &key);
    }
    found
}

pub fn is_verifier(env: &Env, verifier: &Address) -> bool {
    let key = DataKey::Verifier(verifier.clone());
    let found = env.storage().persistent().has(&key);
    if found {
        bump(env, &key);
    }
    found
}

pub fn is_recovery_operator(env: &Env, operator: &Address) -> bool {
    let key = DataKey::RecoveryOperator(operator.clone());
    let found = env.storage().persistent().has(&key);
    if found {
        bump(env, &key);
    }
    found
}

// --- registry writes -------------------------------------------------------

/// Shared body: admin-only, reject a redundant add, store `()` (presence is
/// the fact), bump.
fn register(env: &Env, key: DataKey) -> Result<(), Error> {
    require_admin(env)?;
    let store = env.storage().persistent();
    if store.has(&key) {
        return Err(Error::AlreadyRegistered);
    }
    store.set(&key, &());
    bump(env, &key);
    bump_instance(env);
    Ok(())
}

/// Shared body: admin-only, reject removing something absent, remove.
fn deregister(env: &Env, key: DataKey) -> Result<(), Error> {
    require_admin(env)?;
    let store = env.storage().persistent();
    if !store.has(&key) {
        return Err(Error::NotRegistered);
    }
    store.remove(&key);
    bump_instance(env);
    Ok(())
}

pub fn add_warehouse(env: &Env, warehouse_id: Symbol) -> Result<(), Error> {
    register(env, DataKey::Warehouse(warehouse_id.clone()))?;
    events::warehouse_added(env, &warehouse_id);
    Ok(())
}

/// Custodian registrations for this warehouse are left in place — the keys
/// are `(warehouse, custodian)` pairs and cannot be enumerated to cascade.
/// They are inert: `mint` requires the warehouse itself to be registered, so
/// an orphaned custodian entry authorises nothing.
pub fn remove_warehouse(env: &Env, warehouse_id: Symbol) -> Result<(), Error> {
    deregister(env, DataKey::Warehouse(warehouse_id.clone()))?;
    events::warehouse_removed(env, &warehouse_id);
    Ok(())
}

pub fn add_custodian(env: &Env, warehouse_id: Symbol, custodian: Address) -> Result<(), Error> {
    if !env
        .storage()
        .persistent()
        .has(&DataKey::Warehouse(warehouse_id.clone()))
    {
        return Err(Error::WarehouseNotRegistered);
    }
    register(
        env,
        DataKey::Custodian(warehouse_id.clone(), custodian.clone()),
    )?;
    events::custodian_added(env, &warehouse_id, &custodian);
    Ok(())
}

pub fn remove_custodian(env: &Env, warehouse_id: Symbol, custodian: Address) -> Result<(), Error> {
    deregister(
        env,
        DataKey::Custodian(warehouse_id.clone(), custodian.clone()),
    )?;
    events::custodian_removed(env, &warehouse_id, &custodian);
    Ok(())
}

pub fn add_verifier(env: &Env, verifier: Address) -> Result<(), Error> {
    register(env, DataKey::Verifier(verifier.clone()))?;
    events::verifier_added(env, &verifier);
    Ok(())
}

pub fn remove_verifier(env: &Env, verifier: Address) -> Result<(), Error> {
    deregister(env, DataKey::Verifier(verifier.clone()))?;
    events::verifier_removed(env, &verifier);
    Ok(())
}

pub fn add_recovery_operator(env: &Env, operator: Address) -> Result<(), Error> {
    register(env, DataKey::RecoveryOperator(operator.clone()))?;
    events::recovery_operator_added(env, &operator);
    Ok(())
}

pub fn remove_recovery_operator(env: &Env, operator: Address) -> Result<(), Error> {
    deregister(env, DataKey::RecoveryOperator(operator.clone()))?;
    events::recovery_operator_removed(env, &operator);
    Ok(())
}
