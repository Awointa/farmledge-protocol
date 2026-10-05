#![cfg(test)]

mod adversarial;
mod anchor;
mod audit;
mod exit;
mod layout;
mod lock;
mod mint;
mod prop_focused;
mod prop_lifecycle;
mod prop_split;
mod reads;
mod recovery;
mod registry;
mod revoke;
mod settlement;
mod ttl;
mod unlock;

use core::cell::Cell;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Bytes, BytesN, Env, String, Symbol,
};

use crate::{
    Leaf, ReceiptContract, ReceiptContractClient, WeightSource, KIND_BALANCE, LEAF_VERSION,
};

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
    /// The next day `anchor_next_day` anchors. Starts at 1: every day up to
    /// the ledger's (`NOW`'s) is in the past, so anchors are never early.
    next_day: Cell<u32>,
    /// The next deposit number `balance_leaf` hands out.
    next_no: Cell<u64>,
}

/// A deployed, initialised contract with one warehouse, one custodian
/// registered for it, and one verifier.
///
/// Leaves `mock_all_auths()` in force. Tests that care about a specific
/// signature call `env.mock_auths(..)` afterwards, which replaces it. The
/// ledger clock starts at `NOW`.
pub fn setup(env: &Env) -> Fixture<'_> {
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);

    let admin = Address::generate(env);
    let contract_id = env.register(ReceiptContract, (admin.clone(),));
    let client = ReceiptContractClient::new(env, &contract_id);

    let custodian = Address::generate(env);
    let verifier = Address::generate(env);
    let farmer = Address::generate(env);
    let warehouse = Symbol::new(env, WAREHOUSE);
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
        next_day: Cell::new(1),
        next_no: Cell::new(1),
    }
}

/// A contract with an admin but no warehouses, custodians or verifiers.
pub fn setup_bare(env: &Env) -> Fixture<'_> {
    env.mock_all_auths();
    env.ledger().set_timestamp(NOW);

    let admin = Address::generate(env);
    let contract_id = env.register(ReceiptContract, (admin.clone(),));
    let client = ReceiptContractClient::new(env, &contract_id);

    let custodian = Address::generate(env);
    let verifier = Address::generate(env);
    let farmer = Address::generate(env);
    let warehouse = Symbol::new(env, WAREHOUSE);

    Fixture {
        client,
        contract_id,
        admin,
        custodian,
        verifier,
        farmer,
        warehouse,
        next_day: Cell::new(1),
        next_no: Cell::new(1),
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
        let mut leaves = [self.balance_leaf(env, deposit_id, weight_kg, bags, source)];
        let proofs = self.anchor_next_day(env, &mut leaves);
        self.client
            .mint_from_anchor(&self.custodian, &self.verifier, &leaves[0], &proofs[0])
    }

    /// A balance leaf for a new lot of `self.farmer`'s, with the next deposit
    /// number, attested by `self.custodian` and `self.verifier`. Its `day`
    /// and `index` are filled in by `anchor_next_day`.
    pub fn balance_leaf(
        &self,
        env: &Env,
        deposit_id: &str,
        kg: u64,
        bags: u32,
        source: WeightSource,
    ) -> Leaf {
        let deposit_no = self.next_no.get();
        self.next_no.set(deposit_no + 1);
        let now = env.ledger().timestamp();
        let mut salt = [0u8; 32];
        salt[..8].copy_from_slice(&deposit_no.to_be_bytes());
        Leaf {
            version: LEAF_VERSION,
            kind: KIND_BALANCE,
            warehouse_id: self.warehouse.clone(),
            day: 0,
            index: 0,
            deposit_no,
            deposit_id: String::from_str(env, deposit_id),
            receipt_id: 0,
            receipt_version: 0,
            owner: self.farmer.clone(),
            commodity: Symbol::new(env, COMMODITY),
            grade: Symbol::new(env, GRADE),
            kg,
            weight_source: source,
            bags,
            moisture_bps: MOISTURE,
            custodian: self.custodian.clone(),
            verifier: self.verifier.clone(),
            deposited_at: now,
            at: now,
            salt: BytesN::from_array(env, &salt),
        }
    }

    /// The day `anchor_next_day` will anchor next.
    pub fn next_day(&self) -> u32 {
        self.next_day.get()
    }

    /// Anchors `leaves` as `leaves[0].warehouse_id`'s next day, signed by
    /// `self.custodian`: sets each leaf's `day` and `index`, and returns each
    /// one's proof, in order.
    pub fn anchor_next_day(
        &self,
        env: &Env,
        leaves: &mut [Leaf],
    ) -> std::vec::Vec<soroban_sdk::Vec<BytesN<32>>> {
        let day = self.next_day.get();
        self.next_day.set(day + 1);
        self.anchor_day(env, day, leaves)
    }

    /// `anchor_next_day` for a given day.
    pub fn anchor_day(
        &self,
        env: &Env,
        day: u32,
        leaves: &mut [Leaf],
    ) -> std::vec::Vec<soroban_sdk::Vec<BytesN<32>>> {
        for (i, leaf) in leaves.iter_mut().enumerate() {
            leaf.day = day;
            leaf.index = i as u32;
        }
        let tree = Tree::new(env, &self.client, leaves);
        self.client.anchor(
            &self.custodian,
            &leaves[0].warehouse_id,
            &day,
            &tree.root(),
            &(leaves.len() as u32),
        );
        (0..leaves.len()).map(|i| tree.proof(env, i)).collect()
    }
}

// --- Merkle trees, built the way the backend builds them -------------------

/// A day's tree: leaf hashes padded to a power of two with `sha256(0x00)`,
/// nodes `sha256(0x01 || left || right)`.
pub struct Tree {
    levels: std::vec::Vec<std::vec::Vec<BytesN<32>>>,
}

pub fn node(env: &Env, left: &BytesN<32>, right: &BytesN<32>) -> BytesN<32> {
    let mut b = Bytes::from_array(env, &[1u8]);
    b.append(&left.clone().into());
    b.append(&right.clone().into());
    env.crypto().sha256(&b).into()
}

impl Tree {
    /// Leaf hashes come from the contract's own `leaf_hash`.
    pub fn new(env: &Env, client: &ReceiptContractClient, leaves: &[Leaf]) -> Tree {
        let hashes: std::vec::Vec<BytesN<32>> =
            leaves.iter().map(|l| client.leaf_hash(l)).collect();
        Tree::of_hashes(env, hashes)
    }

    pub fn of_hashes(env: &Env, mut hashes: std::vec::Vec<BytesN<32>>) -> Tree {
        let pad: BytesN<32> = env.crypto().sha256(&Bytes::from_array(env, &[0u8])).into();
        while !hashes.len().is_power_of_two() {
            hashes.push(pad.clone());
        }
        let mut levels = std::vec![hashes];
        while levels.last().unwrap().len() > 1 {
            let cur = levels.last().unwrap();
            let up = cur.chunks(2).map(|p| node(env, &p[0], &p[1])).collect();
            levels.push(up);
        }
        Tree { levels }
    }

    pub fn root(&self) -> BytesN<32> {
        self.levels.last().unwrap()[0].clone()
    }

    pub fn proof(&self, env: &Env, mut index: usize) -> soroban_sdk::Vec<BytesN<32>> {
        let mut out = soroban_sdk::Vec::new(env);
        for level in &self.levels[..self.levels.len() - 1] {
            out.push_back(level[index ^ 1].clone());
            index /= 2;
        }
        out
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

    /// Completes the sale with `SALE-1`; returns the `sale_id`.
    pub fn complete(&self, env: &Env, receipt_id: u64, buyer: &Address) -> u64 {
        self.f.client.complete_recovery(
            &receipt_id,
            &self.operator,
            buyer,
            &String::from_str(env, "SALE-1"),
        )
    }
}

/// Every receipt allocated so far, in id order. No receipt is ever
/// superseded: a split reduces the parent in place and adds one receipt.
pub fn receipts(f: &Fixture, highest_id: u64) -> std::vec::Vec<crate::Receipt> {
    (1..=highest_id)
        .filter_map(|id| f.client.try_get_receipt(&id).ok().and_then(|r| r.ok()))
        .collect()
}

/// Total book weight across every receipt — exited ones included, since an
/// exit retires a receipt without changing its book weight.
pub fn total_weight(f: &Fixture, highest_id: u64) -> u64 {
    receipts(f, highest_id).iter().map(|r| r.weight_kg).sum()
}

/// Total bag count across every receipt.
pub fn total_bags(f: &Fixture, highest_id: u64) -> u32 {
    receipts(f, highest_id).iter().map(|r| r.bag_count).sum()
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
