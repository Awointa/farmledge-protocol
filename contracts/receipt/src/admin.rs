use soroban_sdk::{Address, Env, Symbol};

use crate::errors::Error;
use crate::storage::{bump_instance, DataKey};

pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Admin)
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn require_admin(env: &Env) -> Result<(), Error> {
    let admin = get_admin(env).ok_or(Error::NotInitialized)?;
    admin.require_auth();
    Ok(())
}

pub fn is_custodian(env: &Env, warehouse_id: &Symbol, custodian: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Custodian(warehouse_id.clone(), custodian.clone()))
        .unwrap_or(false)
}

pub fn is_verifier(env: &Env, verifier: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Verifier(verifier.clone()))
        .unwrap_or(false)
}

pub fn is_recovery_operator(env: &Env, operator: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::RecoveryOperator(operator.clone()))
        .unwrap_or(false)
}

/// Admin multisig (2-of-3) registers a custodian for a specific warehouse
/// (design doc §1.2). Body is a TODO: only the auth check is real.
pub fn add_custodian(env: &Env, _warehouse_id: Symbol, _custodian: Address) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if already registered; persist DataKey::Custodian; events::custodian_added.
    todo!("register custodian for warehouse")
}

pub fn remove_custodian(
    env: &Env,
    _warehouse_id: Symbol,
    _custodian: Address,
) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if not registered; remove DataKey::Custodian; events::custodian_removed.
    todo!("deregister custodian for warehouse")
}

/// Farmledge verification agent who co-signs every mint (design doc §3.2).
pub fn add_verifier(env: &Env, _verifier: Address) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if already registered; persist DataKey::Verifier; events::verifier_added.
    todo!("register verifier")
}

pub fn remove_verifier(env: &Env, _verifier: Address) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if not registered; remove DataKey::Verifier; events::verifier_removed.
    todo!("deregister verifier")
}

/// Farmledge staff who manage recovery sales and co-sign completion.
pub fn add_recovery_operator(env: &Env, _operator: Address) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if already registered; persist DataKey::RecoveryOperator; events::recovery_operator_added.
    todo!("register recovery operator")
}

pub fn remove_recovery_operator(env: &Env, _operator: Address) -> Result<(), Error> {
    require_admin(env)?;
    bump_instance(env);
    // TODO: reject if not registered; remove DataKey::RecoveryOperator; events::recovery_operator_removed.
    todo!("deregister recovery operator")
}
