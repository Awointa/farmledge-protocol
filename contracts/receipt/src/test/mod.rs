#![cfg(test)]

mod exit;
mod lock;
mod mint;
mod prop_lending;
mod prop_split;
mod recovery;
mod registry;
mod revoke;
mod ttl;
mod unlock;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String, Symbol,
};

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
        self.mint_with(env, deposit_id, weight_kg, bags, WeightSource::Scale)
    }

    /// Mints a 2,000kg / 20-bag lot whose weight is a bag-count estimate.
    pub fn mint_bag_estimate(&self, env: &Env, deposit_id: &str) -> u64 {
        self.mint_with(env, deposit_id, 2_000, 20, WeightSource::BagEstimate)
    }

    fn mint_with(
        &self,
        env: &Env,
        deposit_id: &str,
        weight_kg: u64,
        bags: u32,
        source: WeightSource,
    ) -> u64 {
        self.client.mint(
            &self.custodian,
            &self.verifier,
            &String::from_str(env, deposit_id),
            &self.farmer,
            &self.warehouse,
            &Symbol::new(env, COMMODITY),
            &Symbol::new(env, GRADE),
            &weight_kg,
            &source,
            &bags,
            &MOISTURE,
        )
    }
}

// --- lending ---------------------------------------------------------------

/// Ledger time every lending test starts at.
pub const NOW: u64 = 1_700_000_000;
/// Loan falls due 30 days after `NOW`...
pub const DUE: u64 = NOW + 30 * 86_400;
/// ...with 7 days' grace, so recovery opens at `DUE + GRACE + 1`.
pub const GRACE: u64 = 7 * 86_400;
pub const LOAN_REF: &str = "LOAN-0001";

/// A `setup` fixture plus a lender and a registered recovery operator, with
/// the ledger clock at `NOW`.
pub struct Lending<'a> {
    pub f: Fixture<'a>,
    pub lender: Address,
    pub operator: Address,
}

pub fn setup_lending(env: &Env) -> Lending<'_> {
    let f = setup(env);
    env.ledger().set_timestamp(NOW);
    let lender = Address::generate(env);
    let operator = Address::generate(env);
    f.client.add_recovery_operator(&operator);
    Lending {
        f,
        lender,
        operator,
    }
}

impl Lending<'_> {
    /// Locks `kg` of `receipt_id` to `self.lender` on the standard terms.
    pub fn lock(&self, env: &Env, receipt_id: u64, kg: u64) -> u64 {
        self.f.client.lock(
            &receipt_id,
            &self.lender,
            &kg,
            &String::from_str(env, LOAN_REF),
            &DUE,
            &GRACE,
        )
    }

    /// Advances the clock past the grace period and starts recovery.
    pub fn default_and_recover(&self, env: &Env, receipt_id: u64) {
        env.ledger().set_timestamp(DUE + GRACE + 1);
        self.f.client.start_recovery(&receipt_id);
    }

    pub fn complete(&self, env: &Env, receipt_id: u64, buyer: &Address) {
        self.f.client.complete_recovery(
            &receipt_id,
            &self.operator,
            buyer,
            &String::from_str(env, "SALE-1"),
            &String::from_str(env, "SETTLE-1"),
        );
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

/// Replaces all mocked auth with exactly `signers` authorising one call of
/// `fn_name(args)`. Anyone not listed has not signed.
pub fn sign_only(
    env: &Env,
    contract: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
    signers: &[&Address],
) {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    let invoke = MockAuthInvoke {
        contract,
        fn_name,
        args,
        sub_invokes: &[],
    };
    let auths: std::vec::Vec<MockAuth> = signers
        .iter()
        .map(|address| MockAuth {
            address,
            invoke: &invoke,
        })
        .collect();
    env.mock_auths(&auths);
}

/// One past the highest receipt id allocated so far.
pub fn next_id(f: &Fixture) -> u64 {
    let mut id = 1;
    while f.client.try_get_receipt(&id).is_ok() {
        id += 1;
    }
    id
}
