//! One named regression test per flaw in the audit of A+B
//! (`docs/storage-strategy.md`, "Audit of A+B"), written before the code
//! that fixes it, and a test for each sound check that has to stay sound.
//!
//! Each flaw's test is named `a<N>_...` after its finding. The rule they all
//! enforce: **a lot's identity and quantity come only from the proven leaf,
//! and uniqueness is keyed only on `(warehouse_id, deposit_no)`** — never on
//! a leaf hash, leaf index, day or root.

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Bytes, BytesN, Env, IntoVal, Symbol, Vec,
};

use super::{node, setup, Fixture, Tree, NOW};
use crate::storage::DataKey;
use crate::{Error, Leaf, WeightSource, KIND_DEPOSIT, KIND_DISPATCH};

fn mint(f: &Fixture, leaf: &Leaf, proof: &Vec<BytesN<32>>) -> Result<u64, Error> {
    match f
        .client
        .try_mint_from_anchor(&f.custodian, &f.verifier, leaf, proof)
    {
        Ok(Ok(id)) => Ok(id),
        Err(Ok(e)) => Err(e),
        other => panic!("unexpected result {other:?}"),
    }
}

fn scale_leaf(env: &Env, f: &Fixture, id: &str, kg: u64) -> Leaf {
    f.balance_leaf(env, id, kg, kg as u32 / 100, WeightSource::Scale)
}

/// The same lot's leaf on a later day: same `deposit_no`, new salt, new day,
/// and whatever position it lands in.
fn again(env: &Env, leaf: &Leaf, kg: u64) -> Leaf {
    let mut next = leaf.clone();
    next.kg = kg;
    next.salt = BytesN::from_array(env, &[0xAB; 32]);
    next
}

// --- A1: the deposit guard is keyed on the proven lot ----------------------

/// The flaw D's prototype had, and A's prototype in another form: a guard
/// keyed on anything but the lot's stable identity lets one lot mint twice.
/// Here the lot is re-anchored the next day (as every active lot is) at a
/// different position, and the backend did not mark it as minted.
#[test]
fn a1_one_lot_cannot_be_minted_twice_through_leaves_from_two_days() {
    let env = Env::default();
    let f = setup(&env);

    let lot = scale_leaf(&env, &f, "DEP-A1", 2_000);
    let mut day1 = [lot.clone()];
    let proofs = f.anchor_next_day(&env, &mut day1);
    let first = mint(&f, &day1[0], &proofs[0]).unwrap();

    // Next day: another lot first, so this one moves to index 1.
    let other = scale_leaf(&env, &f, "DEP-OTHER", 500);
    let mut day2 = [other, again(&env, &lot, 2_000)];
    let proofs = f.anchor_next_day(&env, &mut day2);
    assert_ne!(day2[1].index, day1[0].index);

    assert_eq!(
        mint(&f, &day2[1], &proofs[1]),
        Err(Error::DuplicateDepositId)
    );
    assert!(f.client.is_claimed(&f.warehouse, &lot.deposit_no));
    assert_eq!(f.client.get_receipt(&first).deposit_no, lot.deposit_no);
    // The other lot on that day is untouched by the guard.
    assert!(mint(&f, &day2[0], &proofs[0]).is_ok());
}

/// Two leaves for one lot in the same day's tree (a backend bug, or worse):
/// the guard does not care which position a proof is for.
#[test]
fn a1_one_lot_cannot_be_minted_twice_from_two_leaves_of_one_day() {
    let env = Env::default();
    let f = setup(&env);

    let lot = scale_leaf(&env, &f, "DEP-A1", 2_000);
    let mut day = [lot.clone(), again(&env, &lot, 2_000)];
    let proofs = f.anchor_next_day(&env, &mut day);

    assert!(mint(&f, &day[0], &proofs[0]).is_ok());
    assert_eq!(
        mint(&f, &day[1], &proofs[1]),
        Err(Error::DuplicateDepositId)
    );
}

/// The key is `(warehouse_id, deposit_no)`: deposit numbers are per
/// warehouse, so the same number in another warehouse is another lot.
#[test]
fn a1_the_same_deposit_no_in_another_warehouse_is_another_lot() {
    let env = Env::default();
    let f = setup(&env);
    let other_wh = Symbol::new(&env, "WH_ZARIA_01");
    f.client.add_warehouse(&other_wh);
    f.client.add_custodian(&other_wh, &f.custodian);

    let here = scale_leaf(&env, &f, "DEP-1", 1_000);
    let mut there = here.clone();
    there.warehouse_id = other_wh.clone();

    let mut a = [here];
    let pa = f.anchor_next_day(&env, &mut a);
    let mut b = [there];
    let pb = f.anchor_day(&env, a[0].day, &mut b);

    assert!(mint(&f, &a[0], &pa[0]).is_ok());
    assert!(mint(&f, &b[0], &pb[0]).is_ok());
}

// --- A5: only a balance leaf mints -------------------------------------------

/// A lot deposited at 2,000 kg and since drawn down to 500: its deposit leaf
/// still says 2,000, and must not mint.
#[test]
fn a5_a_deposit_or_dispatch_leaf_cannot_mint() {
    let env = Env::default();
    let f = setup(&env);

    let balance = scale_leaf(&env, &f, "DEP-A5", 500);
    let mut deposit = again(&env, &balance, 2_000);
    deposit.kind = KIND_DEPOSIT;
    let mut dispatch = again(&env, &balance, 1_500);
    dispatch.kind = KIND_DISPATCH;
    dispatch.salt = BytesN::from_array(&env, &[0xCD; 32]);

    let mut day = [deposit, dispatch, balance];
    let proofs = f.anchor_next_day(&env, &mut day);

    assert_eq!(mint(&f, &day[0], &proofs[0]), Err(Error::WrongLeafKind));
    assert_eq!(mint(&f, &day[1], &proofs[1]), Err(Error::WrongLeafKind));
    let id = mint(&f, &day[2], &proofs[2]).unwrap();
    assert_eq!(f.client.get_receipt(&id).weight_kg, 500);
}

// --- A6: only the latest anchored day mints ----------------------------------

/// Yesterday's balance was 2,000; today's anchored balance, after a
/// dispatch, is 500. Yesterday's root is still on-chain, and yesterday's
/// leaf must not mint.
#[test]
fn a6_a_balance_leaf_from_an_earlier_day_cannot_mint() {
    let env = Env::default();
    let f = setup(&env);

    let lot = scale_leaf(&env, &f, "DEP-A6", 2_000);
    let mut yesterday = [lot.clone()];
    let p1 = f.anchor_next_day(&env, &mut yesterday);
    let mut today = [again(&env, &lot, 500)];
    let p2 = f.anchor_next_day(&env, &mut today);

    assert!(
        f.client.verify_leaf(&yesterday[0], &p1[0]),
        "yesterday's root is still on-chain"
    );
    assert_eq!(mint(&f, &yesterday[0], &p1[0]), Err(Error::StaleLeaf));
    let id = mint(&f, &today[0], &p2[0]).unwrap();
    assert_eq!(f.client.get_receipt(&id).weight_kg, 500);
}

// --- A8: every receipt field comes from the proven leaf ----------------------

/// The receipt is the leaf: including who attested the intake and when it
/// happened, which are not who signs the mint or when.
#[test]
fn a8_every_receipt_field_comes_from_the_proven_leaf() {
    let env = Env::default();
    let f = setup(&env);
    let intake_custodian = Address::generate(&env);
    let intake_verifier = Address::generate(&env);
    f.client.add_custodian(&f.warehouse, &intake_custodian);
    f.client.add_verifier(&intake_verifier);

    let mut lot = f.balance_leaf(&env, "DEP-A8", 1_234, 13, WeightSource::Scale);
    lot.owner = Address::generate(&env);
    lot.custodian = intake_custodian.clone();
    lot.verifier = intake_verifier.clone();
    lot.commodity = Symbol::new(&env, "SORGHUM");
    lot.grade = Symbol::new(&env, "GRADE_2");
    lot.moisture_bps = 1_375;
    lot.deposited_at = NOW - 40 * 86_400;
    lot.deposit_no = 777;
    let mut day = [lot];
    let proofs = f.anchor_next_day(&env, &mut day);
    let leaf = &day[0];

    env.ledger().set_timestamp(NOW + 5_000);
    let id = mint(&f, leaf, &proofs[0]).unwrap();
    let r = f.client.get_receipt(&id);

    assert_eq!(r.deposit_no, leaf.deposit_no);
    assert_eq!(r.deposit_id, leaf.deposit_id);
    assert_eq!(r.warehouse_id, leaf.warehouse_id);
    assert_eq!(r.owner, leaf.owner);
    assert_eq!(r.commodity, leaf.commodity);
    assert_eq!(r.grade, leaf.grade);
    assert_eq!(r.weight_kg, leaf.kg);
    assert_eq!(r.weight_source, leaf.weight_source);
    assert_eq!(r.bag_count, leaf.bags);
    assert_eq!(r.moisture_pct, leaf.moisture_bps);
    assert_eq!(
        r.custodian, intake_custodian,
        "the intake's custodian, not the mint's"
    );
    assert_eq!(
        r.verifier, intake_verifier,
        "the intake's verifier, not the mint's"
    );
    assert_eq!(
        r.deposit_date, leaf.deposited_at,
        "the intake's date, not the mint's"
    );
}

/// The proof binds every field: change any one after anchoring and the mint
/// is refused.
#[test]
fn a8_a_leaf_altered_after_anchoring_cannot_mint() {
    let env = Env::default();
    let f = setup(&env);

    let mut day = [scale_leaf(&env, &f, "DEP-A8", 1_000)];
    let proofs = f.anchor_next_day(&env, &mut day);
    let leaf = day[0].clone();

    let alterations: [fn(&Env, &mut Leaf); 6] = [
        |_, l| l.kg += 1,
        |_, l| l.bags += 1,
        |env, l| l.owner = Address::generate(env),
        |env, l| l.grade = Symbol::new(env, "GRADE_2"),
        |_, l| l.weight_source = WeightSource::BagEstimate,
        |_, l| l.deposit_no += 1,
    ];
    for alter in alterations {
        let mut forged = leaf.clone();
        alter(&env, &mut forged);
        assert_eq!(mint(&f, &forged, &proofs[0]), Err(Error::InvalidProof));
    }
    assert!(mint(&f, &leaf, &proofs[0]).is_ok());
}

// --- A11: anchors are strictly ordered and never ahead of the ledger ---------

/// "Never anchored" is the head's absence, not day 0: day 0 anchors once.
#[test]
fn a11_day_zero_is_not_a_never_anchored_sentinel() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[1; 32]);

    f.client.anchor(&f.custodian, &f.warehouse, &0, &root, &1);
    let other = BytesN::from_array(&env, &[2; 32]);
    assert_eq!(
        f.client
            .try_anchor(&f.custodian, &f.warehouse, &0, &other, &1),
        Err(Ok(Error::AnchorOutOfOrder))
    );
    assert_eq!(f.client.get_anchor(&f.warehouse).day, 0);
}

/// A far-future day would lock out every real day, and point every mint at
/// a day that has not happened.
#[test]
fn a11_a_day_after_the_ledger_day_cannot_be_anchored() {
    let env = Env::default();
    let f = setup(&env);
    let today = (NOW / 86_400) as u32;
    let root = BytesN::from_array(&env, &[1; 32]);

    for day in [today + 1, today + 365, u32::MAX] {
        assert_eq!(
            f.client
                .try_anchor(&f.custodian, &f.warehouse, &day, &root, &1),
            Err(Ok(Error::AnchorInFuture))
        );
    }
    assert_eq!(
        f.client.try_get_anchor(&f.warehouse),
        Err(Ok(Error::NoAnchor))
    );
    // Today is not in the future, and anchoring it does not depend on what
    // was attempted before.
    f.client
        .anchor(&f.custodian, &f.warehouse, &today, &root, &1);
    // Tomorrow becomes anchorable once the ledger reaches it, not before.
    assert_eq!(
        f.client
            .try_anchor(&f.custodian, &f.warehouse, &(today + 1), &root, &1),
        Err(Ok(Error::AnchorInFuture))
    );
    env.ledger().set_timestamp((today as u64 + 1) * 86_400);
    f.client
        .anchor(&f.custodian, &f.warehouse, &(today + 1), &root, &1);
}

// --- A19: lots held on an earlier contract are pre-claimed -------------------

/// A lot still held as a receipt on the `data_version` 2 contract appears in
/// v3's balance leaves. Pre-claimed, it can never mint here — even if the
/// backend leaves `receipt_id` unset.
#[test]
fn a19_a_preclaimed_lot_can_never_be_minted() {
    let env = Env::default();
    let f = setup(&env);

    let lot = scale_leaf(&env, &f, "DEP-V2", 2_000);
    f.client
        .preclaim(&f.warehouse, &Vec::from_array(&env, [lot.deposit_no]));
    assert!(f.client.is_claimed(&f.warehouse, &lot.deposit_no));

    let mut day = [lot];
    let proofs = f.anchor_next_day(&env, &mut day);
    assert_eq!(
        mint(&f, &day[0], &proofs[0]),
        Err(Error::DuplicateDepositId)
    );
}

/// Admin only, all-or-nothing, and a number already claimed is an error
/// rather than a silent no-op, because it means the list is wrong.
#[test]
fn a19_preclaim_is_admin_only_and_rejects_a_number_already_claimed() {
    let env = Env::default();
    let f = setup(&env);
    let nos = Vec::from_array(&env, [10u64, 11, 12]);

    super::sign_only(
        &env,
        &f.contract_id,
        "preclaim",
        (f.warehouse.clone(), nos.clone()).into_val(&env),
        &[&f.custodian],
    );
    assert!(f.client.try_preclaim(&f.warehouse, &nos).is_err());
    env.mock_all_auths();
    assert!(!f.client.is_claimed(&f.warehouse, &10));

    assert_eq!(f.client.preclaim(&f.warehouse, &nos), 3);
    let overlapping = Vec::from_array(&env, [13u64, 12]);
    assert_eq!(
        f.client.try_preclaim(&f.warehouse, &overlapping),
        Err(Ok(Error::DuplicateDepositId))
    );
    assert!(!f.client.is_claimed(&f.warehouse, &13), "all-or-nothing");
}

/// The leaf's own `receipt_id` is checked too, as defence in depth: a lot
/// the books show as governed by a receipt is not minted again.
#[test]
fn a19_a_leaf_naming_a_receipt_cannot_mint() {
    let env = Env::default();
    let f = setup(&env);

    // Either field on its own is enough.
    let mut both = scale_leaf(&env, &f, "DEP-V2", 2_000);
    (both.receipt_id, both.receipt_version) = (41, 2);
    let mut id_only = scale_leaf(&env, &f, "DEP-V2-ID", 2_000);
    id_only.receipt_id = 42;
    let mut version_only = scale_leaf(&env, &f, "DEP-V2-VER", 2_000);
    version_only.receipt_version = 2;
    let mut day = [both, id_only, version_only];
    let proofs = f.anchor_next_day(&env, &mut day);
    for (leaf, proof) in day.iter().zip(&proofs) {
        assert_eq!(mint(&f, leaf, proof), Err(Error::LeafHasReceipt));
        assert!(!f.client.is_claimed(&f.warehouse, &leaf.deposit_no));
    }
}

// --- sound checks that must stay sound ---------------------------------------

/// A3: the guard must never move to temporary storage, where expiry would
/// reopen every number in the chunk.
#[test]
fn a3_the_claim_bitmap_is_persistent_storage() {
    let env = Env::default();
    let f = setup(&env);
    f.client
        .preclaim(&f.warehouse, &Vec::from_array(&env, [5u64, 1_029]));

    env.as_contract(&f.contract_id, || {
        for chunk in [0u64, 1] {
            let key = DataKey::Claimed(f.warehouse.clone(), chunk);
            assert!(env.storage().persistent().has(&key), "chunk {chunk}");
            assert!(!env.storage().temporary().has(&key));
            // One bit per number: 128 bytes, whatever the rent would bear.
            let bits: Bytes = env.storage().persistent().get(&key).unwrap();
            assert_eq!(bits.len(), 128);
        }
    });
}

/// A4: 1,024 numbers per chunk, and neighbours do not interfere.
#[test]
fn a4_claims_land_in_their_own_bit() {
    let env = Env::default();
    let f = setup(&env);
    f.client
        .preclaim(&f.warehouse, &Vec::from_array(&env, [0u64, 1_023, 1_024]));
    for (n, claimed) in [
        (0u64, true),
        (1, false),
        (1_022, false),
        (1_023, true),
        (1_024, true),
        (1_025, false),
    ] {
        assert_eq!(
            f.client.is_claimed(&f.warehouse, &n),
            claimed,
            "deposit_no {n}"
        );
    }
}

/// A10: a proof must be exactly as long as the day's tree is deep, and
/// for a position inside it.
#[test]
fn a10_a_proof_of_the_wrong_length_or_position_is_rejected() {
    let env = Env::default();
    let f = setup(&env);

    let mut day = [
        scale_leaf(&env, &f, "DEP-1", 1_000),
        scale_leaf(&env, &f, "DEP-2", 1_000),
        scale_leaf(&env, &f, "DEP-3", 1_000),
    ];
    let proofs = f.anchor_next_day(&env, &mut day);
    let leaf = &day[2];

    let mut short = proofs[2].clone();
    short.pop_back();
    let mut long = proofs[2].clone();
    long.push_back(BytesN::from_array(&env, &[0; 32]));
    assert_eq!(mint(&f, leaf, &short), Err(Error::InvalidProof));
    assert_eq!(mint(&f, leaf, &long), Err(Error::InvalidProof));

    // Index 3 is the padding leaf of a 3-leaf day: out of range.
    let mut padded = leaf.clone();
    padded.index = 3;
    assert_eq!(mint(&f, &padded, &proofs[2]), Err(Error::InvalidProof));
    assert!(mint(&f, leaf, &proofs[2]).is_ok());
}

/// A10: the fold only reads as many bits of `index` as the tree is deep, so
/// a leaf whose `index` lies outside its day (a backend bug) can still fold
/// to the root. The range check refuses it on its own.
#[test]
fn a10_a_leaf_indexed_outside_its_day_is_rejected_even_when_the_proof_folds() {
    let env = Env::default();
    let f = setup(&env);
    let day = f.next_day();

    // Two leaves; the first sits at position 0 but says index 2, which
    // folds exactly like 0 in a one-level tree.
    let mut odd = scale_leaf(&env, &f, "DEP-1", 1_000);
    let mut other = scale_leaf(&env, &f, "DEP-2", 1_000);
    (odd.day, odd.index) = (day, 2);
    (other.day, other.index) = (day, 1);
    let tree = Tree::new(&env, &f.client, &[odd.clone(), other]);
    f.client
        .anchor(&f.custodian, &f.warehouse, &day, &tree.root(), &2);
    let proof = tree.proof(&env, 0);

    assert_eq!(mint(&f, &odd, &proof), Err(Error::InvalidProof));
    assert!(!f.client.verify_leaf(&odd, &proof));
}

/// A10: an inner node of an anchored tree is not a leaf. Leaf hashes are
/// `sha256(0x00 || XDR(leaf))` and nodes `sha256(0x01 || left || right)`, so
/// no leaf can hash to a node; the contract's `leaf_hash` uses the prefix.
#[test]
fn a10_an_inner_node_cannot_pass_as_a_leaf() {
    let env = Env::default();
    let f = setup(&env);
    let a = BytesN::from_array(&env, &[1; 32]);
    let b = BytesN::from_array(&env, &[2; 32]);
    let tree = Tree::of_hashes(&env, std::vec![a.clone(), b.clone()]);
    assert_eq!(tree.root(), node(&env, &a, &b));

    // The leaf prefix and the node prefix differ, so no leaf hashes to a
    // node: leaf_hash(x) is sha256(0x00 || x), node is sha256(0x01 || ..).
    let lot = scale_leaf(&env, &f, "DEP-1", 1_000);
    let mut pre = Bytes::from_array(&env, &[0u8]);
    pre.append(&soroban_sdk::xdr::ToXdr::to_xdr(lot.clone(), &env));
    let expected: BytesN<32> = env.crypto().sha256(&pre).into();
    assert_eq!(f.client.leaf_hash(&lot), expected);
}

/// A12: once a day is anchored, no root replaces it, not even the same day
/// again with a different root.
#[test]
fn a12_an_anchored_day_cannot_be_replaced() {
    let env = Env::default();
    let f = setup(&env);
    let mut day = [scale_leaf(&env, &f, "DEP-1", 1_000)];
    let proofs = f.anchor_next_day(&env, &mut day);
    let d = day[0].day;

    for earlier_or_same in [d, d - 1] {
        assert_eq!(
            f.client.try_anchor(
                &f.custodian,
                &f.warehouse,
                &earlier_or_same,
                &BytesN::from_array(&env, &[9; 32]),
                &1
            ),
            Err(Ok(Error::AnchorOutOfOrder))
        );
    }
    assert!(f.client.verify_leaf(&day[0], &proofs[0]));
}

/// A13: only a custodian registered for that warehouse, now, anchors it.
#[test]
fn a13_only_a_custodian_of_the_warehouse_can_anchor_it() {
    let env = Env::default();
    let f = setup(&env);
    let root = BytesN::from_array(&env, &[1; 32]);
    let stranger = Address::generate(&env);
    let other_wh = Symbol::new(&env, "WH_ZARIA_01");
    f.client.add_warehouse(&other_wh);
    let theirs = Address::generate(&env);
    f.client.add_custodian(&other_wh, &theirs);

    assert_eq!(
        f.client.try_anchor(&stranger, &f.warehouse, &1, &root, &1),
        Err(Ok(Error::CustodianNotRegistered))
    );
    assert_eq!(
        f.client.try_anchor(&theirs, &f.warehouse, &1, &root, &1),
        Err(Ok(Error::CustodianNotRegistered))
    );
    super::sign_only(
        &env,
        &f.contract_id,
        "anchor",
        (
            theirs.clone(),
            f.warehouse.clone(),
            1u32,
            root.clone(),
            1u32,
        )
            .into_val(&env),
        &[&theirs],
    );
    assert!(
        f.client
            .try_anchor(&f.custodian, &f.warehouse, &1, &root, &1)
            .is_err(),
        "the named custodian must sign"
    );
    env.mock_all_auths();

    f.client.remove_custodian(&f.warehouse, &f.custodian);
    assert_eq!(
        f.client
            .try_anchor(&f.custodian, &f.warehouse, &1, &root, &1),
        Err(Ok(Error::CustodianNotRegistered))
    );
}

/// A14: the head is persistent, so ordering survives; A15: the day root is
/// temporary and lives 30 days.
#[test]
fn a14_a15_the_head_is_persistent_and_the_day_root_temporary() {
    let env = Env::default();
    let f = setup(&env);
    let mut day = [scale_leaf(&env, &f, "DEP-1", 1_000)];
    f.anchor_next_day(&env, &mut day);
    let d = day[0].day;

    env.as_contract(&f.contract_id, || {
        let head = DataKey::Head(f.warehouse.clone());
        let root = DataKey::DayRoot(f.warehouse.clone(), d);
        assert!(env.storage().persistent().has(&head));
        assert!(!env.storage().temporary().has(&head));
        assert!(env.storage().temporary().has(&root));
        assert!(!env.storage().persistent().has(&root));
        assert_eq!(
            soroban_sdk::testutils::storage::Temporary::get_ttl(&env.storage().temporary(), &root),
            crate::storage::DAY_ROOT_TTL
        );
    });
    // 30 days, in ledgers: still there on day 29.
    assert_eq!(crate::storage::DAY_ROOT_TTL, 30 * 17_280);
    env.ledger().with_mut(|l| l.sequence_number += 29 * 17_280);
    env.as_contract(&f.contract_id, || {
        let root = DataKey::DayRoot(f.warehouse.clone(), d);
        assert!(
            env.storage().temporary().has(&root),
            "a day root lasts 30 days"
        );
    });
}

/// A16: `head = sha256(prev || day || leaves || root)`, from 32 zero bytes,
/// so anyone can recompute the chain from the `Anchored` events.
#[test]
fn a16_the_head_chains_every_anchored_day() {
    let env = Env::default();
    let f = setup(&env);
    let r1 = BytesN::from_array(&env, &[1; 32]);
    let r2 = BytesN::from_array(&env, &[2; 32]);

    let chain = |prev: &BytesN<32>, day: u32, leaves: u32, root: &BytesN<32>| -> BytesN<32> {
        let mut b: Bytes = prev.clone().into();
        b.append(&Bytes::from_array(&env, &day.to_be_bytes()));
        b.append(&Bytes::from_array(&env, &leaves.to_be_bytes()));
        b.append(&root.clone().into());
        env.crypto().sha256(&b).into()
    };
    let h1 = chain(&BytesN::from_array(&env, &[0; 32]), 7, 3, &r1);
    let h2 = chain(&h1, 9, 5, &r2);

    assert_eq!(f.client.anchor(&f.custodian, &f.warehouse, &7, &r1, &3), h1);
    assert_eq!(f.client.anchor(&f.custodian, &f.warehouse, &9, &r2, &5), h2);
    let head = f.client.get_anchor(&f.warehouse);
    assert_eq!((head.day, head.head), (9, h2));
}
