//! THROWAWAY PROTOTYPE for docs/storage-strategy.md. Measures what designs
//! A (mint on demand), B (daily anchoring), C (aggregated lots) and D
//! (minimal chain) would store and cost. Not production code; never deploy
//! as Farmledge.

use soroban_sdk::{
    contractevent, contractimpl, contractmeta, contracttype, xdr::ToXdr, Address, Bytes, BytesN,
    Env, String, Symbol, Vec,
};

use crate::admin;
use crate::errors::Error;
use crate::layout::StoredLock;
use crate::storage::{self, ENTRY_TTL_EXTEND_TO, ENTRY_TTL_THRESHOLD};
use crate::types::{MaybeLock, MaybeRecovery, Receipt, Status, WeightSource, DATA_VERSION};
use crate::{ReceiptContract, ReceiptContractArgs, ReceiptContractClient};

contractmeta!(
    key = "Description",
    val = "THROWAWAY PROTOTYPE for Farmledge storage-strategy cost measurement. Not a Farmledge deployment. Do not use."
);

const BITS_PER_CHUNK: u64 = 1024;

#[contracttype]
pub enum ProtoKey {
    /// A, C: duplicate guard, one bit per deposit number, 1,024 per entry.
    Minted(Symbol, u64),
    /// B: one day's root and the chain head before it.
    DayRoot(Symbol, u32),
    /// B: the running hash chain over every day's root.
    Head(Symbol),
    /// C: every active lot of one farmer, warehouse and commodity.
    Agg(Address, Symbol, Symbol),
    /// D: one pledge.
    Pledge(u64),
    NextPledge,
    /// D: leaves already pledged, one bit per leaf index.
    LeafUsed(Symbol, u32, u32),
}

/// B: (root, head before this day, leaf count).
#[contracttype]
#[derive(Clone)]
pub struct StoredDay(pub BytesN<32>, pub BytesN<32>, pub u32);

/// B: (last day anchored, head).
#[contracttype]
#[derive(Clone)]
pub struct StoredHead(pub u32, pub BytesN<32>);

/// C: deposit_no, grade, kg, bags, moisture, weight_source code,
/// deposit_date, custodian, verifier, status code, lock (empty or one).
#[contracttype]
#[derive(Clone)]
pub struct Lot(
    pub u64,
    pub Symbol,
    pub u64,
    pub u32,
    pub u32,
    pub u32,
    pub u64,
    pub Address,
    pub Address,
    pub u32,
    pub Vec<StoredLock>,
);

/// D: data_version, warehouse, day, leaf, owner, lender, commodity, grade,
/// kg, loan_ref, due_date, grace_period, status code, created_at.
#[contracttype]
#[derive(Clone)]
pub struct StoredPledge(
    pub u32,
    pub Symbol,
    pub u32,
    pub BytesN<32>,
    pub Address,
    pub Address,
    pub Symbol,
    pub Symbol,
    pub u64,
    pub String,
    pub u64,
    pub u64,
    pub u32,
    pub u64,
);

/// B, D: one leaf of a warehouse's daily tree. `kind`: 0 deposit,
/// 1 dispatch, 2 end-of-day balance. Hashed as sha256(0x00 || XDR).
#[contracttype]
#[derive(Clone)]
pub struct Leaf {
    pub version: u32,
    pub kind: u32,
    pub warehouse_id: Symbol,
    pub day: u32,
    pub index: u32,
    pub deposit_id: String,
    pub owner: Address,
    pub commodity: Symbol,
    pub grade: Symbol,
    pub kg: u64,
    pub bags: u32,
    pub moisture_bps: u32,
    pub custodian: Address,
    pub verifier: Address,
    pub at: u64,
    pub salt: BytesN<32>,
}

#[contractevent]
pub struct Anchored {
    #[topic]
    pub warehouse_id: Symbol,
    pub day: u32,
    pub root: BytesN<32>,
    pub head: BytesN<32>,
    pub leaves: u32,
}

#[contractevent]
pub struct Pledged {
    #[topic]
    pub pledge_id: u64,
    #[topic]
    pub lender: Address,
    pub owner: Address,
    pub warehouse_id: Symbol,
    pub day: u32,
    pub leaf: BytesN<32>,
    pub kg: u64,
    pub loan_ref: String,
}

fn bump(env: &Env, key: &ProtoKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
}

/// Sets the bit for `n` in `key_of(n / 1024)`; `false` if it was already set.
fn claim_bit(env: &Env, key: ProtoKey, n: u64) -> bool {
    let store = env.storage().persistent();
    let mut bits: Bytes = store
        .get(&key)
        .unwrap_or(Bytes::from_array(env, &[0u8; 128]));
    let bit = (n % BITS_PER_CHUNK) as u32;
    let byte = bits.get_unchecked(bit / 8);
    let mask = 1u8 << (bit % 8);
    if byte & mask != 0 {
        return false;
    }
    bits.set(bit / 8, byte | mask);
    store.set(&key, &bits);
    bump(env, &key);
    true
}

fn node(env: &Env, left: &BytesN<32>, right: &BytesN<32>) -> BytesN<32> {
    let mut b = Bytes::from_array(env, &[1u8]);
    b.append(&left.clone().into());
    b.append(&right.clone().into());
    env.crypto().sha256(&b).into()
}

fn verify(env: &Env, root: &BytesN<32>, leaf: &BytesN<32>, index: u32, proof: &Vec<BytesN<32>>) -> bool {
    let mut acc = leaf.clone();
    let mut i = index;
    for sibling in proof.iter() {
        acc = if i & 1 == 0 {
            node(env, &acc, &sibling)
        } else {
            node(env, &sibling, &acc)
        };
        i >>= 1;
    }
    &acc == root
}

fn leaf_hash_of(env: &Env, leaf: &Leaf) -> BytesN<32> {
    let mut b = Bytes::from_array(env, &[0u8]);
    b.append(&leaf.clone().to_xdr(env));
    env.crypto().sha256(&b).into()
}

#[contractimpl]
impl ReceiptContract {
    // --- A: mint on demand ---------------------------------------------------

    /// `mint`, with the per-deposit `DepositIndex` guard replaced by one bit
    /// in a per-warehouse bitmap keyed by the database's deposit number.
    #[allow(clippy::too_many_arguments)]
    pub fn a_mint(
        env: Env,
        custodian: Address,
        verifier: Address,
        deposit_id: String,
        deposit_no: u64,
        farmer: Address,
        warehouse_id: Symbol,
        commodity: Symbol,
        grade: Symbol,
        weight_kg: u64,
        weight_source: WeightSource,
        bag_count: u32,
        moisture_pct: u32,
    ) -> Result<u64, Error> {
        custodian.require_auth();
        verifier.require_auth();
        if !admin::is_custodian(&env, &warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        if !admin::is_verifier(&env, &verifier) {
            return Err(Error::VerifierNotRegistered);
        }
        let chunk = ProtoKey::Minted(warehouse_id.clone(), deposit_no / BITS_PER_CHUNK);
        if !claim_bit(&env, chunk, deposit_no) {
            return Err(Error::DuplicateDepositId);
        }
        let receipt_id = storage::next_receipt_id(&env);
        let receipt = Receipt {
            data_version: DATA_VERSION,
            receipt_id,
            deposit_id,
            parent_id: None,
            commodity,
            grade,
            weight_kg,
            weight_source,
            bag_count,
            moisture_pct,
            warehouse_id: warehouse_id.clone(),
            custodian,
            verifier,
            owner: farmer.clone(),
            status: Status::Active,
            lock: MaybeLock::None,
            recovery: MaybeRecovery::None,
            recovery_history: Vec::new(&env),
            deposit_date: env.ledger().timestamp(),
            exit_date: None,
            exit_weight_kg: None,
        };
        storage::set_receipt(&env, &receipt);
        storage::index_owner(&env, &farmer, receipt_id);
        storage::index_warehouse(&env, &warehouse_id, receipt_id);
        storage::bump_instance(&env);
        Ok(receipt_id)
    }

    // --- B: daily anchoring ------------------------------------------------

    /// Anchors one day's root for a warehouse. The head is
    /// sha256(previous head || day || root), so the latest head commits to
    /// every root before it. `keep_day` also stores the day's root on its
    /// own, which on-chain proof checks (design D) need.
    pub fn b_anchor(
        env: Env,
        custodian: Address,
        warehouse_id: Symbol,
        day: u32,
        root: BytesN<32>,
        leaves: u32,
        keep_day: bool,
    ) -> Result<BytesN<32>, Error> {
        custodian.require_auth();
        if !admin::is_custodian(&env, &warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        let store = env.storage().persistent();
        let head_key = ProtoKey::Head(warehouse_id.clone());
        let prev: StoredHead = store
            .get(&head_key)
            .unwrap_or(StoredHead(0, BytesN::from_array(&env, &[0u8; 32])));
        if prev.0 >= day && prev.0 != 0 {
            return Err(Error::InvalidStatus);
        }
        let mut b: Bytes = prev.1.clone().into();
        b.append(&Bytes::from_array(&env, &day.to_be_bytes()));
        b.append(&root.clone().into());
        let head: BytesN<32> = env.crypto().sha256(&b).into();
        if keep_day {
            let key = ProtoKey::DayRoot(warehouse_id.clone(), day);
            store.set(&key, &StoredDay(root.clone(), prev.1, leaves));
            bump(&env, &key);
        }
        store.set(&head_key, &StoredHead(day, head.clone()));
        bump(&env, &head_key);
        storage::bump_instance(&env);
        Anchored {
            warehouse_id,
            day,
            root,
            head: head.clone(),
            leaves,
        }
        .publish(&env);
        Ok(head)
    }

    /// The leaf hash a third party would compute.
    pub fn leaf_hash(env: Env, leaf: Leaf) -> BytesN<32> {
        leaf_hash_of(&env, &leaf)
    }

    /// Whether `leaf` is in the stored root of `warehouse_id` on `day`.
    pub fn b_verify(
        env: Env,
        warehouse_id: Symbol,
        day: u32,
        leaf: BytesN<32>,
        index: u32,
        proof: Vec<BytesN<32>>,
    ) -> bool {
        let day_root: Option<StoredDay> = env
            .storage()
            .persistent()
            .get(&ProtoKey::DayRoot(warehouse_id, day));
        match day_root {
            Some(d) => verify(&env, &d.0, &leaf, index, &proof),
            None => false,
        }
    }

    // --- C: aggregated lots ------------------------------------------------

    /// Adds a lot to the farmer's entry for this warehouse and commodity.
    #[allow(clippy::too_many_arguments)]
    pub fn c_add_lot(
        env: Env,
        custodian: Address,
        verifier: Address,
        farmer: Address,
        warehouse_id: Symbol,
        commodity: Symbol,
        deposit_no: u64,
        grade: Symbol,
        weight_kg: u64,
        bag_count: u32,
        moisture_pct: u32,
    ) -> Result<u32, Error> {
        custodian.require_auth();
        verifier.require_auth();
        if !admin::is_custodian(&env, &warehouse_id, &custodian) {
            return Err(Error::CustodianNotRegistered);
        }
        let chunk = ProtoKey::Minted(warehouse_id.clone(), deposit_no / BITS_PER_CHUNK);
        if !claim_bit(&env, chunk, deposit_no) {
            return Err(Error::DuplicateDepositId);
        }
        let key = ProtoKey::Agg(farmer, warehouse_id, commodity);
        let store = env.storage().persistent();
        let mut lots: Vec<Lot> = store.get(&key).unwrap_or(Vec::new(&env));
        lots.push_back(Lot(
            deposit_no,
            grade,
            weight_kg,
            bag_count,
            moisture_pct,
            1,
            env.ledger().timestamp(),
            custodian,
            verifier,
            0,
            Vec::new(&env),
        ));
        store.set(&key, &lots);
        bump(&env, &key);
        storage::bump_instance(&env);
        Ok(lots.len())
    }

    /// Pledges one lot inside the aggregate.
    #[allow(clippy::too_many_arguments)]
    pub fn c_pledge_lot(
        env: Env,
        farmer: Address,
        lender: Address,
        warehouse_id: Symbol,
        commodity: Symbol,
        deposit_no: u64,
        loan_ref: String,
        due_date: u64,
        grace_period: u64,
    ) -> Result<(), Error> {
        farmer.require_auth();
        lender.require_auth();
        let key = ProtoKey::Agg(farmer, warehouse_id, commodity);
        let store = env.storage().persistent();
        let mut lots: Vec<Lot> = store.get(&key).ok_or(Error::ReceiptNotFound)?;
        for i in 0..lots.len() {
            let mut lot = lots.get_unchecked(i);
            if lot.0 == deposit_no {
                if lot.9 != 0 {
                    return Err(Error::InvalidStatus);
                }
                lot.9 = 1;
                lot.10 = Vec::from_array(
                    &env,
                    [StoredLock(lender.clone(), loan_ref, due_date, grace_period)],
                );
                lots.set(i, lot);
                store.set(&key, &lots);
                bump(&env, &key);
                storage::index_lender(&env, &lender, deposit_no);
                storage::bump_instance(&env);
                return Ok(());
            }
        }
        Err(Error::ReceiptNotFound)
    }

    // --- D: minimal chain ----------------------------------------------------

    /// Creates a pledge from an anchored balance leaf. The contract checks
    /// the leaf is in that day's root and was not pledged before.
    pub fn d_pledge(
        env: Env,
        lender: Address,
        leaf: Leaf,
        proof: Vec<BytesN<32>>,
        loan_ref: String,
        due_date: u64,
        grace_period: u64,
    ) -> Result<u64, Error> {
        leaf.owner.require_auth();
        lender.require_auth();
        let day_root: StoredDay = env
            .storage()
            .persistent()
            .get(&ProtoKey::DayRoot(leaf.warehouse_id.clone(), leaf.day))
            .ok_or(Error::NotRegistered)?;
        let hash = leaf_hash_of(&env, &leaf);
        if !verify(&env, &day_root.0, &hash, leaf.index, &proof) {
            return Err(Error::InvalidStatus);
        }
        let used = ProtoKey::LeafUsed(leaf.warehouse_id.clone(), leaf.day, leaf.index / 1024);
        if !claim_bit(&env, used, leaf.index as u64) {
            return Err(Error::DuplicateDepositId);
        }
        let store = env.storage().instance();
        let id: u64 = store.get(&ProtoKey::NextPledge).unwrap_or(0) + 1;
        store.set(&ProtoKey::NextPledge, &id);
        let key = ProtoKey::Pledge(id);
        env.storage().persistent().set(
            &key,
            &StoredPledge(
                DATA_VERSION,
                leaf.warehouse_id.clone(),
                leaf.day,
                hash.clone(),
                leaf.owner.clone(),
                lender.clone(),
                leaf.commodity.clone(),
                leaf.grade.clone(),
                leaf.kg,
                loan_ref.clone(),
                due_date,
                grace_period,
                1,
                env.ledger().timestamp(),
            ),
        );
        bump(&env, &key);
        storage::index_lender(&env, &lender, id);
        storage::bump_instance(&env);
        Pledged {
            pledge_id: id,
            lender,
            owner: leaf.owner,
            warehouse_id: leaf.warehouse_id,
            day: leaf.day,
            leaf: hash,
            kg: leaf.kg,
            loan_ref,
        }
        .publish(&env);
        Ok(id)
    }

    /// Releases a pledge. The recorded lender signs.
    pub fn d_release(env: Env, pledge_id: u64) -> Result<(), Error> {
        let key = ProtoKey::Pledge(pledge_id);
        let mut p: StoredPledge = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::ReceiptNotFound)?;
        p.5.require_auth();
        if p.12 != 1 {
            return Err(Error::InvalidStatus);
        }
        p.12 = 0;
        env.storage().persistent().set(&key, &p);
        bump(&env, &key);
        storage::bump_instance(&env);
        Ok(())
    }
}
