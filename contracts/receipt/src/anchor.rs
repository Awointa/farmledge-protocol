//! Daily anchoring and minting on demand (`data_version` 3).
//!
//! Each warehouse commits its books once a day: a Merkle root over that
//! day's deposit, dispatch and end-of-day balance leaves, chained into a
//! head. A receipt is minted only when a lot needs one (to be pledged), from
//! its balance leaf in the latest anchored day, with an inclusion proof.
//! Design and audit: `docs/storage-strategy.md`.
//!
//! **The rule every check here follows:** a lot's identity and quantity come
//! only from the proven leaf, and uniqueness is keyed only on the lot's
//! stable identity `(warehouse_id, deposit_no)` — never on a leaf hash, leaf
//! index, day or root, all of which change from day to day. The audit
//! findings each check closes are named beside it (A1–A21).

use soroban_sdk::{xdr::ToXdr, Address, Bytes, BytesN, Env, Symbol, Vec};

use crate::admin;
use crate::errors::Error;
use crate::events;
use crate::lending::MAX_REF_LEN;
use crate::storage::{self, StoredDay, StoredHead};
use crate::types::{
    AnchorHead, Leaf, MaybeLock, MaybeRecovery, Receipt, Status, WeightSource, DATA_VERSION,
};
use crate::{MAX_MOISTURE_BPS, MAX_TTL_BATCH};

/// The only `Leaf::version` this contract accepts.
pub const LEAF_VERSION: u32 = 1;
/// `Leaf::kind`: a lot taken in that day.
pub const KIND_DEPOSIT: u32 = 0;
/// `Leaf::kind`: grain dispatched from a lot that day.
pub const KIND_DISPATCH: u32 = 1;
/// `Leaf::kind`: what a lot holds at the day's cutoff. The only kind that
/// mints.
pub const KIND_BALANCE: u32 = 2;

/// Deepest tree `anchor` accepts: 2^20 = 1,048,576 leaves a day, far above
/// any warehouse, and it bounds the cost of a proof check.
pub const MAX_TREE_DEPTH: u32 = 20;

const SECONDS_PER_DAY: u64 = 86_400;

/// The ledger's current UTC day. Consensus time, so every validator agrees.
pub fn ledger_day(env: &Env) -> u32 {
    (env.ledger().timestamp() / SECONDS_PER_DAY) as u32
}

/// `sha256(0x00 || XDR(leaf))`. The prefix keeps a leaf from ever being
/// taken for a node (A10).
pub fn leaf_hash(env: &Env, leaf: &Leaf) -> BytesN<32> {
    let mut b = Bytes::from_array(env, &[0u8]);
    b.append(&leaf.clone().to_xdr(env));
    env.crypto().sha256(&b).into()
}

/// `sha256(0x01 || left || right)`.
fn node(env: &Env, left: &BytesN<32>, right: &BytesN<32>) -> BytesN<32> {
    let mut b = Bytes::from_array(env, &[1u8]);
    b.append(&left.clone().into());
    b.append(&right.clone().into());
    env.crypto().sha256(&b).into()
}

/// Depth of the tree a day of `leaves` leaves is padded to: the smallest
/// `d` with `2^d >= leaves`. `leaves` is at least 1.
fn depth(leaves: u32) -> u32 {
    u32::BITS - (leaves - 1).leading_zeros()
}

/// Whether `hash` is leaf `index` of the day `day` describes. The proof must
/// be exactly as long as that day's tree is deep, and `index` inside it, so
/// a proof can neither stop short at an inner node nor reach a padding leaf
/// (A9, A10).
fn proves(
    env: &Env,
    day: &StoredDay,
    hash: &BytesN<32>,
    index: u32,
    proof: &Vec<BytesN<32>>,
) -> bool {
    if index >= day.1 || proof.len() != depth(day.1) {
        return false;
    }
    let mut acc = hash.clone();
    let mut i = index;
    for sibling in proof.iter() {
        acc = if i & 1 == 0 {
            node(env, &acc, &sibling)
        } else {
            node(env, &sibling, &acc)
        };
        i >>= 1;
    }
    acc == day.0
}

/// `sha256(prev || day || leaves || root)`, `day` and `leaves` big-endian.
fn chain(env: &Env, prev: &BytesN<32>, day: u32, leaves: u32, root: &BytesN<32>) -> BytesN<32> {
    let mut b: Bytes = prev.clone().into();
    b.append(&Bytes::from_array(env, &day.to_be_bytes()));
    b.append(&Bytes::from_array(env, &leaves.to_be_bytes()));
    b.append(&root.clone().into());
    env.crypto().sha256(&b).into()
}

pub fn anchor(
    env: &Env,
    custodian: Address,
    warehouse_id: Symbol,
    day: u32,
    root: BytesN<32>,
    leaves: u32,
) -> Result<BytesN<32>, Error> {
    // A13: a custodian of this warehouse, registered now.
    custodian.require_auth();
    if !admin::is_warehouse(env, &warehouse_id) {
        return Err(Error::WarehouseNotRegistered);
    }
    if !admin::is_custodian(env, &warehouse_id, &custodian) {
        return Err(Error::CustodianNotRegistered);
    }
    if leaves == 0 || leaves > 1 << MAX_TREE_DEPTH {
        return Err(Error::InvalidLeafCount);
    }

    // A11, A12: strictly after the last anchored day — "never anchored" is
    // the head's absence, not day 0 — and never after today.
    let prev = storage::get_head(env, &warehouse_id);
    if let Some(p) = &prev {
        if day <= p.0 {
            return Err(Error::AnchorOutOfOrder);
        }
    }
    if day > ledger_day(env) {
        return Err(Error::AnchorInFuture);
    }

    // A14, A16: the head is persistent and commits to every day before it.
    let prev_head = match prev {
        Some(p) => p.1,
        None => BytesN::from_array(env, &[0u8; 32]),
    };
    let head = chain(env, &prev_head, day, leaves, &root);
    storage::set_head(env, &warehouse_id, &StoredHead(day, head.clone()));
    // A15: the root itself only needs to be on-chain while mints use it.
    storage::set_day_root(env, &warehouse_id, day, &StoredDay(root.clone(), leaves));
    storage::bump_instance(env);

    events::Anchored {
        warehouse_id,
        day,
        leaves,
        root,
        prev_head,
        head: head.clone(),
        custodian,
        anchored_at: env.ledger().timestamp(),
    }
    .publish(env);
    Ok(head)
}

pub fn mint_from_anchor(
    env: &Env,
    custodian: Address,
    verifier: Address,
    leaf: Leaf,
    proof: Vec<BytesN<32>>,
) -> Result<u64, Error> {
    // Both signatures first, so neither can be skipped by tripping a later
    // validation error.
    custodian.require_auth();
    verifier.require_auth();

    let warehouse_id = leaf.warehouse_id.clone();
    if !admin::is_warehouse(env, &warehouse_id) {
        return Err(Error::WarehouseNotRegistered);
    }
    if !admin::is_custodian(env, &warehouse_id, &custodian) {
        return Err(Error::CustodianNotRegistered);
    }
    if !admin::is_verifier(env, &verifier) {
        return Err(Error::VerifierNotRegistered);
    }

    if leaf.version != LEAF_VERSION {
        return Err(Error::UnsupportedLeafVersion);
    }
    // A5: a deposit or dispatch leaf records a movement, not what the lot
    // holds now.
    if leaf.kind != KIND_BALANCE {
        return Err(Error::WrongLeafKind);
    }
    // A lot the books already show as governed by a receipt, here or on an
    // earlier contract, is not minted again. Defence in depth only: the
    // guard below does not rely on the backend filling this in (A19).
    if leaf.receipt_id != 0 || leaf.receipt_version != 0 {
        return Err(Error::LeafHasReceipt);
    }
    // A6: only the latest anchored day's balance is current. The contract
    // cannot see dispatches made since that anchor; the backend freezes a
    // lot's dispatches while its mint is pending (docs/storage-strategy.md).
    let head = storage::get_head(env, &warehouse_id).ok_or(Error::NoAnchor)?;
    if leaf.day != head.0 {
        return Err(Error::StaleLeaf);
    }
    let day = storage::get_day_root(env, &warehouse_id, leaf.day).ok_or(Error::NoAnchor)?;

    if leaf.kg == 0 {
        return Err(Error::InvalidWeight);
    }
    // A bag-estimated weight with no bags is not an estimate of anything.
    if leaf.weight_source == WeightSource::BagEstimate && leaf.bags == 0 {
        return Err(Error::InvalidWeight);
    }
    if leaf.moisture_bps > MAX_MOISTURE_BPS {
        return Err(Error::InvalidMoisture);
    }
    if leaf.deposit_id.len() > MAX_REF_LEN {
        return Err(Error::RefTooLong);
    }

    // A9, A10: the leaf, every field of it, is in that day's books.
    let hash = leaf_hash(env, &leaf);
    if !proves(env, &day, &hash, leaf.index, &proof) {
        return Err(Error::InvalidProof);
    }

    // A1, A3, A4, A7: one bit per lot, keyed on the proven leaf's stable
    // identity — the same bit through every day's leaf, index and root.
    if !storage::claim(env, &warehouse_id, leaf.deposit_no) {
        return Err(Error::DuplicateDepositId);
    }

    // A8: every field from the proven leaf. The mint's signers are recorded
    // in the event; the receipt keeps who attested the intake.
    let receipt_id = storage::next_receipt_id(env);
    let receipt = Receipt {
        data_version: DATA_VERSION,
        receipt_id,
        deposit_id: leaf.deposit_id.clone(),
        deposit_no: leaf.deposit_no,
        parent_id: None,
        commodity: leaf.commodity.clone(),
        grade: leaf.grade.clone(),
        weight_kg: leaf.kg,
        weight_source: leaf.weight_source.clone(),
        bag_count: leaf.bags,
        moisture_pct: leaf.moisture_bps,
        warehouse_id: warehouse_id.clone(),
        custodian: leaf.custodian.clone(),
        verifier: leaf.verifier.clone(),
        owner: leaf.owner.clone(),
        status: Status::Active,
        lock: MaybeLock::None,
        recovery: MaybeRecovery::None,
        recovery_history: Vec::new(env),
        deposit_date: leaf.deposited_at,
        exit_date: None,
        exit_weight_kg: None,
    };

    storage::set_receipt(env, &receipt);
    storage::index_owner(env, &receipt.owner, receipt_id);
    storage::index_warehouse(env, &warehouse_id, receipt_id);
    storage::bump_instance(env);

    events::Minted {
        receipt_id,
        warehouse_id,
        deposit_no: receipt.deposit_no,
        deposit_id: receipt.deposit_id,
        owner: receipt.owner,
        commodity: receipt.commodity,
        grade: receipt.grade,
        weight_kg: receipt.weight_kg,
        weight_source: receipt.weight_source,
        bag_count: receipt.bag_count,
        moisture_pct: receipt.moisture_pct,
        custodian: receipt.custodian,
        verifier: receipt.verifier,
        deposit_date: receipt.deposit_date,
        minted_by_custodian: custodian,
        minted_by_verifier: verifier,
        anchor_day: leaf.day,
        leaf_hash: hash,
        minted_at: env.ledger().timestamp(),
    }
    .publish(env);
    Ok(receipt_id)
}

pub fn preclaim(env: &Env, warehouse_id: Symbol, deposit_nos: Vec<u64>) -> Result<u32, Error> {
    admin::require_admin(env)?;
    if deposit_nos.len() > MAX_TTL_BATCH {
        return Err(Error::BatchTooLarge);
    }
    if !admin::is_warehouse(env, &warehouse_id) {
        return Err(Error::WarehouseNotRegistered);
    }
    let now = env.ledger().timestamp();
    for deposit_no in deposit_nos.iter() {
        // All-or-nothing: one number already claimed fails the whole call,
        // because it means the list is wrong.
        if !storage::claim(env, &warehouse_id, deposit_no) {
            return Err(Error::DuplicateDepositId);
        }
        events::Preclaimed {
            warehouse_id: warehouse_id.clone(),
            deposit_no,
            claimed_at: now,
        }
        .publish(env);
    }
    storage::bump_instance(env);
    Ok(deposit_nos.len())
}

/// Whether `leaf` is in its warehouse's anchored root for `leaf.day`, by
/// `proof`. `false` once that day's root has expired; older days are checked
/// off-chain through the head chain (`docs/events.md`).
pub fn verify_leaf(env: &Env, leaf: &Leaf, proof: &Vec<BytesN<32>>) -> bool {
    match storage::get_day_root(env, &leaf.warehouse_id, leaf.day) {
        Some(day) => proves(env, &day, &leaf_hash(env, leaf), leaf.index, proof),
        None => false,
    }
}

pub fn get_anchor(env: &Env, warehouse_id: &Symbol) -> Result<AnchorHead, Error> {
    let head = storage::get_head(env, warehouse_id).ok_or(Error::NoAnchor)?;
    Ok(AnchorHead {
        day: head.0,
        head: head.1,
    })
}
