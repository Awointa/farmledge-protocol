#![cfg(test)]

mod exit;
mod mint;
mod prop_split;
mod registry;
mod revoke;
mod ttl;

use soroban_sdk::{testutils::Address as _, Address, Env, String, Symbol};

use crate::{ReceiptContract, ReceiptContractClient, Status, WeightSource};

pub const WAREHOUSE: &str = "WH_KANO_01";
pub const COMMODITY: &str = "MAIZE_WHITE";
pub const GRADE: &str = "GRADE_1";
/// 12.50% in basis points of a percent.
pub const MOISTURE: u32 = 1250;

pub struct Fixture<'a> {
    pub client: ReceiptContractClient<'a>,
    pub contract_id: Address,
    pub admin: Address,
    pub custodian: Address,
    pub verifier: Address,
    pub farmer: Address,
    pub warehouse: Symbol,
}

/// A deployed, initialised contract with one warehouse, one custodian
/// registered for it, and one verifier.
///
/// Leaves `mock_all_auths()` in force. Tests that care about a specific
/// signature call `env.mock_auths(..)` afterwards, which replaces it.
pub fn setup(env: &Env) -> Fixture<'_> {
    env.mock_all_auths();

    let contract_id = env.register(ReceiptContract, ());
    let client = ReceiptContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let custodian = Address::generate(env);
    let verifier = Address::generate(env);
    let farmer = Address::generate(env);
    let warehouse = Symbol::new(env, WAREHOUSE);

    client.initialize(&admin);
    client.add_warehouse(&warehouse);
    client.add_custodian(&warehouse, &custodian);
    client.add_verifier(&verifier);

    Fixture {
        client,
        contract_id,
        admin,
        custodian,
        verifier,
        farmer,
        warehouse,
    }
}

/// A contract with an admin but no warehouses, custodians or verifiers.
pub fn setup_bare(env: &Env) -> Fixture<'_> {
    env.mock_all_auths();

    let contract_id = env.register(ReceiptContract, ());
    let client = ReceiptContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let custodian = Address::generate(env);
    let verifier = Address::generate(env);
    let farmer = Address::generate(env);
    let warehouse = Symbol::new(env, WAREHOUSE);

    client.initialize(&admin);

    Fixture {
        client,
        contract_id,
        admin,
        custodian,
        verifier,
        farmer,
        warehouse,
    }
}

impl Fixture<'_> {
    /// Mints a 2,000kg / 20-bag scale-weighed lot.
    pub fn mint(&self, env: &Env, deposit_id: &str) -> u64 {
        self.mint_weighing(env, deposit_id, 2_000, 20)
    }

    pub fn mint_weighing(&self, env: &Env, deposit_id: &str, weight_kg: u64, bags: u32) -> u64 {
        self.client.mint(
            &self.custodian,
            &self.verifier,
            &String::from_str(env, deposit_id),
            &self.farmer,
            &self.warehouse,
            &Symbol::new(env, COMMODITY),
            &Symbol::new(env, GRADE),
            &weight_kg,
            &WeightSource::Scale,
            &bags,
            &MOISTURE,
        )
    }
}

/// Every receipt that is not a retired `Split` parent — i.e. the leaves of
/// the split tree plus any unsplit receipts. Split parents are excluded
/// because their weight is already accounted for by their two children.
pub fn leaves(f: &Fixture, highest_id: u64) -> std::vec::Vec<crate::Receipt> {
    (1..=highest_id)
        .filter_map(|id| f.client.try_get_receipt(&id).ok().and_then(|r| r.ok()))
        .filter(|r| r.status != Status::Split)
        .collect()
}

/// Total book weight still accounted for across the split tree.
pub fn live_weight(f: &Fixture, highest_id: u64) -> u64 {
    leaves(f, highest_id).iter().map(|r| r.weight_kg).sum()
}

/// Total bag count still accounted for across the split tree.
pub fn live_bags(f: &Fixture, highest_id: u64) -> u32 {
    leaves(f, highest_id).iter().map(|r| r.bag_count).sum()
}
