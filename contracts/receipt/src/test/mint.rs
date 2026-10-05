//! `mint_from_anchor`: who may mint, what it validates, what it writes.
//! The checks the audit of A+B depends on have their own named tests in
//! `audit.rs`.

use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, BytesN, Env, IntoVal, String, Symbol, Vec,
};

use super::{setup, setup_bare, Fixture, MOISTURE, NOW, WAREHOUSE};
use crate::{Error, Leaf, MaybeLock, MaybeRecovery, Status, WeightSource};

#[test]
fn mint_creates_an_active_receipt() {
    let env = Env::default();
    let f = setup(&env);

    let id = f.mint(&env, "DEP-0001");
    let r = f.client.get_receipt(&id);

    assert_eq!(r.receipt_id, id);
    assert_eq!(r.status, Status::Active);
    assert_eq!(r.weight_kg, 2_000);
    assert_eq!(r.bag_count, 20);
    assert_eq!(r.weight_source, WeightSource::Scale);
    assert_eq!(r.owner, f.farmer);
    assert_eq!(r.custodian, f.custodian);
    assert_eq!(r.verifier, f.verifier);
    assert_eq!(r.moisture_pct, MOISTURE);
    assert_eq!(r.parent_id, None);
    assert_eq!(r.deposit_no, 1);
    assert_eq!(r.deposit_date, NOW);
    assert_eq!(r.data_version, 3);
    assert_eq!(r.exit_date, None);
    assert_eq!(r.exit_weight_kg, None);
    // Lending state exists on the receipt from day one, always empty here.
    assert_eq!(r.lock, MaybeLock::None);
    assert_eq!(r.recovery, MaybeRecovery::None);
}

#[test]
fn mint_indexes_the_receipt_by_owner_and_warehouse() {
    let env = Env::default();
    let f = setup(&env);

    let a = f.mint(&env, "DEP-0001");
    let b = f.mint(&env, "DEP-0002");

    let by_owner = f.client.get_receipts_by_owner(&f.farmer, &0);
    assert_eq!(by_owner.len(), 2);
    assert_eq!(by_owner.get(0).unwrap().receipt_id, a);
    assert_eq!(by_owner.get(1).unwrap().receipt_id, b);

    let by_warehouse = f.client.get_receipts_by_warehouse(&f.warehouse, &0);
    assert_eq!(by_warehouse.len(), 2);

    assert_eq!(f.client.owner_page_count(&f.farmer), 1);
    assert_eq!(f.client.warehouse_page_count(&f.warehouse), 1);

    // A farmer with no receipts reads as empty, not as an error.
    let stranger = Address::generate(&env);
    assert_eq!(f.client.owner_page_count(&stranger), 0);
    assert_eq!(f.client.get_receipts_by_owner(&stranger, &0).len(), 0);
}

/// The index must roll onto a second page once the first fills, and the
/// rollover must not drop or duplicate an id.
#[test]
fn owner_index_pages_roll_over_at_64() {
    let env = Env::default();
    let f = setup(&env);

    for i in 0..70 {
        let mut buf = std::string::String::from("DEP-");
        buf.push_str(&std::format!("{i:04}"));
        f.mint(&env, &buf);
    }

    assert_eq!(f.client.owner_page_count(&f.farmer), 2);
    assert_eq!(f.client.get_receipts_by_owner(&f.farmer, &0).len(), 64);
    assert_eq!(f.client.get_receipts_by_owner(&f.farmer, &1).len(), 6);

    // Ids 1..=70, each appearing exactly once across the two pages.
    let mut seen = std::vec::Vec::new();
    for page in 0..2 {
        for r in f.client.get_receipts_by_owner(&f.farmer, &page).iter() {
            seen.push(r.receipt_id);
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, (1..=70u64).collect::<std::vec::Vec<_>>());
}

// --- authorisation ---------------------------------------------------------

/// One lot anchored as the next day; returns its leaf and proof.
fn anchored(env: &Env, f: &Fixture, deposit_id: &str) -> (Leaf, Vec<BytesN<32>>) {
    let mut day = [f.balance_leaf(env, deposit_id, 2_000, 20, WeightSource::Scale)];
    let proofs = f.anchor_next_day(env, &mut day);
    (day[0].clone(), proofs[0].clone())
}

fn mint_auth<'a>(
    f: &'a Fixture,
    args: &'a soroban_sdk::Vec<soroban_sdk::Val>,
) -> MockAuthInvoke<'a> {
    MockAuthInvoke {
        contract: &f.contract_id,
        fn_name: "mint_from_anchor",
        args: args.clone(),
        sub_invokes: &[],
    }
}

/// Only the custodian signs. The verifier's `require_auth()` must trap, so a
/// single warehouse employee cannot conjure a receipt alone.
#[test]
fn mint_fails_with_only_the_custodian_signature() {
    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-SOLO-C");
    let args = (f.custodian.clone(), f.verifier.clone(), leaf.clone(), proof.clone()).into_val(&env);
    let invoke = mint_auth(&f, &args);

    env.mock_auths(&[MockAuth { address: &f.custodian, invoke: &invoke }]);
    assert!(f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof).is_err());
}

/// Only the verifier signs. The custodian's `require_auth()` must trap.
#[test]
fn mint_fails_with_only_the_verifier_signature() {
    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-SOLO-V");
    let args = (f.custodian.clone(), f.verifier.clone(), leaf.clone(), proof.clone()).into_val(&env);
    let invoke = mint_auth(&f, &args);

    env.mock_auths(&[MockAuth { address: &f.verifier, invoke: &invoke }]);
    assert!(f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof).is_err());
}

/// The farmer cannot mint their own lot, even holding its proof.
#[test]
fn mint_fails_with_only_the_farmers_signature() {
    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-FARMER");
    let args = (f.custodian.clone(), f.verifier.clone(), leaf.clone(), proof.clone()).into_val(&env);
    let invoke = mint_auth(&f, &args);

    env.mock_auths(&[MockAuth { address: &f.farmer, invoke: &invoke }]);
    assert!(f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof).is_err());
}

/// Both sign: the same call that failed above now succeeds, which is what
/// makes those failures meaningful.
#[test]
fn mint_succeeds_with_both_signatures() {
    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-BOTH");
    let args = (f.custodian.clone(), f.verifier.clone(), leaf.clone(), proof.clone()).into_val(&env);
    let invoke = mint_auth(&f, &args);

    env.mock_auths(&[
        MockAuth { address: &f.custodian, invoke: &invoke },
        MockAuth { address: &f.verifier, invoke: &invoke },
    ]);
    assert_eq!(f.client.mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof), 1);
}

// --- validation ------------------------------------------------------------

/// A custodian registered for warehouse A cannot mint a lot of warehouse B.
#[test]
fn mint_rejects_a_custodian_registered_for_another_warehouse() {
    let env = Env::default();
    let f = setup(&env);

    let other_warehouse = Symbol::new(&env, "WH_KADUNA_02");
    let other_custodian = Address::generate(&env);
    f.client.add_warehouse(&other_warehouse);
    f.client.add_custodian(&other_warehouse, &other_custodian);

    let (leaf, proof) = anchored(&env, &f, "DEP-XWH");
    assert_eq!(
        f.client.try_mint_from_anchor(&other_custodian, &f.verifier, &leaf, &proof),
        Err(Ok(Error::CustodianNotRegistered))
    );
    assert!(f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof).is_ok());
}

#[test]
fn mint_rejects_an_unregistered_warehouse() {
    let env = Env::default();
    let f = setup(&env);
    let (mut leaf, proof) = anchored(&env, &f, "DEP-NOWH");
    leaf.warehouse_id = Symbol::new(&env, "WH_NOWHERE");
    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof),
        Err(Ok(Error::WarehouseNotRegistered))
    );

    let bare = setup_bare(&env);
    let leaf = Leaf { warehouse_id: Symbol::new(&env, WAREHOUSE), ..leaf };
    assert_eq!(
        bare.client.try_mint_from_anchor(&bare.custodian, &bare.verifier, &leaf, &proof),
        Err(Ok(Error::WarehouseNotRegistered))
    );
}

#[test]
fn mint_rejects_an_unregistered_verifier() {
    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-NOV");
    let stranger = Address::generate(&env);
    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &stranger, &leaf, &proof),
        Err(Ok(Error::VerifierNotRegistered))
    );
}

#[test]
fn mint_rejects_a_warehouse_never_anchored() {
    let env = Env::default();
    let f = setup(&env);
    let mut leaf = f.balance_leaf(&env, "DEP-0", 2_000, 20, WeightSource::Scale);
    leaf.day = 1;
    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &Vec::new(&env)),
        Err(Ok(Error::NoAnchor))
    );
}

/// Day roots are temporary: once the latest one has expired (a warehouse
/// not anchored for 30 days), nothing mints until the next anchor.
#[test]
fn mint_rejects_a_leaf_whose_day_root_has_expired() {
    use soroban_sdk::testutils::Ledger as _;

    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-OLD");
    env.ledger().with_mut(|l| l.sequence_number += crate::storage::DAY_ROOT_TTL + 1);
    assert!(!f.client.verify_leaf(&leaf, &proof));
    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof),
        Err(Ok(Error::NoAnchor))
    );
}

#[test]
fn mint_rejects_an_unknown_leaf_version() {
    let env = Env::default();
    let f = setup(&env);
    let mut leaf = f.balance_leaf(&env, "DEP-V", 2_000, 20, WeightSource::Scale);
    leaf.version = 2;
    let mut day = [leaf];
    let proofs = f.anchor_next_day(&env, &mut day);
    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &day[0], &proofs[0]),
        Err(Ok(Error::UnsupportedLeafVersion))
    );
}

#[test]
fn mint_rejects_invalid_measurements() {
    let env = Env::default();
    let f = setup(&env);

    let leaf = |id: &str, kg: u64, source: WeightSource, bags: u32, moisture: u32| {
        let mut l = f.balance_leaf(&env, id, kg, bags, source);
        l.moisture_bps = moisture;
        l
    };
    let mut day = [
        leaf("DEP-W0", 0, WeightSource::Scale, 20, MOISTURE),
        leaf("DEP-B0", 2_000, WeightSource::BagEstimate, 0, MOISTURE),
        leaf("DEP-M", 2_000, WeightSource::Scale, 20, 10_001),
        leaf("DEP-M-MAX", 2_000, WeightSource::Scale, 20, 10_000),
        leaf("DEP-BULK", 2_000, WeightSource::Scale, 0, MOISTURE),
        leaf(&"D".repeat(65), 2_000, WeightSource::Scale, 20, MOISTURE),
        leaf(&"D".repeat(64), 2_000, WeightSource::Scale, 20, MOISTURE),
    ];
    let proofs = f.anchor_next_day(&env, &mut day);
    let mint = |i: usize| f.client.try_mint_from_anchor(&f.custodian, &f.verifier, &day[i], &proofs[i]);

    assert_eq!(mint(0), Err(Ok(Error::InvalidWeight)), "zero weight");
    assert_eq!(mint(1), Err(Ok(Error::InvalidWeight)), "bag estimate with no bags");
    assert_eq!(mint(2), Err(Ok(Error::InvalidMoisture)), "moisture above 100.00%");
    // The boundary itself is accepted: 100.00% is a reading, not an error.
    assert!(mint(3).is_ok());
    // A scale-weighed lot with no bag count is legitimate (bulk intake).
    assert!(mint(4).is_ok());
    assert_eq!(mint(5), Err(Ok(Error::RefTooLong)), "deposit_id over 64 bytes");
    assert!(mint(6).is_ok());
}

/// A rejected mint claims nothing: the lot can still mint once the problem
/// is fixed.
#[test]
fn a_rejected_mint_leaves_the_lot_unclaimed() {
    let env = Env::default();
    let f = setup(&env);
    let mut day = [
        f.balance_leaf(&env, "DEP-RETRY", 2_000, 20, WeightSource::Scale),
        f.balance_leaf(&env, "DEP-OTHER", 2_000, 20, WeightSource::Scale),
    ];
    let proofs = f.anchor_next_day(&env, &mut day);
    let (leaf, proof) = (&day[0], &proofs[0]);
    let mut wrong = proof.clone();
    wrong.set(0, BytesN::from_array(&env, &[7; 32]));

    assert_eq!(
        f.client.try_mint_from_anchor(&f.custodian, &f.verifier, leaf, &wrong),
        Err(Ok(Error::InvalidProof))
    );
    let stranger = Address::generate(&env);
    assert!(f.client.try_mint_from_anchor(&f.custodian, &stranger, leaf, proof).is_err());
    assert!(!f.client.is_claimed(&f.warehouse, &leaf.deposit_no));
    assert!(f.client.try_mint_from_anchor(&f.custodian, &f.verifier, leaf, proof).is_ok());
}

#[test]
fn mint_emits_a_minted_event() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let (leaf, proof) = anchored(&env, &f, "DEP-EVT");
    let hash = f.client.leaf_hash(&leaf);
    let id = f.client.mint_from_anchor(&f.custodian, &f.verifier, &leaf, &proof);

    let expected = crate::events::Minted {
        receipt_id: id,
        warehouse_id: f.warehouse.clone(),
        deposit_no: leaf.deposit_no,
        deposit_id: String::from_str(&env, "DEP-EVT"),
        owner: f.farmer.clone(),
        commodity: Symbol::new(&env, super::COMMODITY),
        grade: Symbol::new(&env, super::GRADE),
        weight_kg: 2_000,
        weight_source: WeightSource::Scale,
        bag_count: 20,
        moisture_pct: MOISTURE,
        custodian: f.custodian.clone(),
        verifier: f.verifier.clone(),
        deposit_date: NOW,
        minted_by_custodian: f.custodian.clone(),
        minted_by_verifier: f.verifier.clone(),
        anchor_day: leaf.day,
        leaf_hash: hash,
        minted_at: NOW,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &f.contract_id)]
    );
}
