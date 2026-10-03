use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, String, Symbol,
};

use super::{setup, setup_bare, COMMODITY, GRADE, MOISTURE, WAREHOUSE};
use crate::{Error, MaybeLock, MaybeRecovery, Status, WeightSource};

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

/// Builds the exact mint invocation, so a `MockAuth` can be scoped to it.
fn mint_args(
    env: &Env,
    f: &super::Fixture,
    deposit_id: &str,
) -> soroban_sdk::Vec<soroban_sdk::Val> {
    (
        f.custodian.clone(),
        f.verifier.clone(),
        String::from_str(env, deposit_id),
        f.farmer.clone(),
        f.warehouse.clone(),
        Symbol::new(env, COMMODITY),
        Symbol::new(env, GRADE),
        2_000u64,
        WeightSource::Scale,
        20u32,
        MOISTURE,
    )
        .into_val(env)
}

/// Only the custodian signs. The verifier's `require_auth()` must trap, so a
/// single warehouse employee cannot conjure a receipt alone.
#[test]
fn mint_fails_with_only_the_custodian_signature() {
    let env = Env::default();
    let f = setup(&env);
    let args = mint_args(&env, &f, "DEP-SOLO-C");

    env.mock_auths(&[MockAuth {
        address: &f.custodian,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "mint",
            args: args.clone(),
            sub_invokes: &[],
        },
    }]);

    assert!(f
        .client
        .try_mint(
            &f.custodian,
            &f.verifier,
            &String::from_str(&env, "DEP-SOLO-C"),
            &f.farmer,
            &f.warehouse,
            &Symbol::new(&env, COMMODITY),
            &Symbol::new(&env, GRADE),
            &2_000u64,
            &WeightSource::Scale,
            &20u32,
            &MOISTURE,
        )
        .is_err());
}

/// Only the verifier signs. The custodian's `require_auth()` must trap.
#[test]
fn mint_fails_with_only_the_verifier_signature() {
    let env = Env::default();
    let f = setup(&env);
    let args = mint_args(&env, &f, "DEP-SOLO-V");

    env.mock_auths(&[MockAuth {
        address: &f.verifier,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "mint",
            args,
            sub_invokes: &[],
        },
    }]);

    assert!(f
        .client
        .try_mint(
            &f.custodian,
            &f.verifier,
            &String::from_str(&env, "DEP-SOLO-V"),
            &f.farmer,
            &f.warehouse,
            &Symbol::new(&env, COMMODITY),
            &Symbol::new(&env, GRADE),
            &2_000u64,
            &WeightSource::Scale,
            &20u32,
            &MOISTURE,
        )
        .is_err());
}

/// Both sign: the same call that failed twice above now succeeds, which is
/// what makes the two failures above meaningful.
#[test]
fn mint_succeeds_with_both_signatures() {
    let env = Env::default();
    let f = setup(&env);
    let args = mint_args(&env, &f, "DEP-BOTH");

    env.mock_auths(&[
        MockAuth {
            address: &f.custodian,
            invoke: &MockAuthInvoke {
                contract: &f.contract_id,
                fn_name: "mint",
                args: args.clone(),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &f.verifier,
            invoke: &MockAuthInvoke {
                contract: &f.contract_id,
                fn_name: "mint",
                args,
                sub_invokes: &[],
            },
        },
    ]);

    let id = f.client.mint(
        &f.custodian,
        &f.verifier,
        &String::from_str(&env, "DEP-BOTH"),
        &f.farmer,
        &f.warehouse,
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(id, 1);
}

// --- validation ------------------------------------------------------------

#[test]
fn mint_rejects_a_duplicate_deposit_id() {
    let env = Env::default();
    let f = setup(&env);

    f.mint(&env, "DEP-0001");

    let again = f.client.try_mint(
        &f.custodian,
        &f.verifier,
        &String::from_str(&env, "DEP-0001"),
        &f.farmer,
        &f.warehouse,
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(again, Err(Ok(Error::DuplicateDepositId)));
}

/// A custodian registered for warehouse A cannot mint against warehouse B.
#[test]
fn mint_rejects_a_custodian_registered_for_another_warehouse() {
    let env = Env::default();
    let f = setup(&env);

    let other_warehouse = Symbol::new(&env, "WH_KADUNA_02");
    let other_custodian = Address::generate(&env);
    f.client.add_warehouse(&other_warehouse);
    f.client.add_custodian(&other_warehouse, &other_custodian);

    // f.custodian is registered for WAREHOUSE, not for other_warehouse.
    let res = f.client.try_mint(
        &f.custodian,
        &f.verifier,
        &String::from_str(&env, "DEP-XWH"),
        &f.farmer,
        &other_warehouse,
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(res, Err(Ok(Error::CustodianNotRegistered)));

    // ...and the other custodian cannot mint against WAREHOUSE either.
    let res = f.client.try_mint(
        &other_custodian,
        &f.verifier,
        &String::from_str(&env, "DEP-XWH2"),
        &f.farmer,
        &f.warehouse,
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(res, Err(Ok(Error::CustodianNotRegistered)));
}

#[test]
fn mint_rejects_an_unregistered_warehouse() {
    let env = Env::default();
    let f = setup_bare(&env);

    let res = f.client.try_mint(
        &f.custodian,
        &f.verifier,
        &String::from_str(&env, "DEP-NOWH"),
        &f.farmer,
        &Symbol::new(&env, WAREHOUSE),
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(res, Err(Ok(Error::WarehouseNotRegistered)));
}

#[test]
fn mint_rejects_an_unregistered_verifier() {
    let env = Env::default();
    let f = setup(&env);

    let stranger = Address::generate(&env);
    let res = f.client.try_mint(
        &f.custodian,
        &stranger,
        &String::from_str(&env, "DEP-NOV"),
        &f.farmer,
        &f.warehouse,
        &Symbol::new(&env, COMMODITY),
        &Symbol::new(&env, GRADE),
        &2_000u64,
        &WeightSource::Scale,
        &20u32,
        &MOISTURE,
    );
    assert_eq!(res, Err(Ok(Error::VerifierNotRegistered)));
}

#[test]
fn mint_rejects_invalid_measurements() {
    let env = Env::default();
    let f = setup(&env);

    let mint = |deposit: &str, weight: u64, source: WeightSource, bags: u32, moisture: u32| {
        f.client.try_mint(
            &f.custodian,
            &f.verifier,
            &String::from_str(&env, deposit),
            &f.farmer,
            &f.warehouse,
            &Symbol::new(&env, COMMODITY),
            &Symbol::new(&env, GRADE),
            &weight,
            &source,
            &bags,
            &moisture,
        )
    };

    assert_eq!(
        mint("DEP-W0", 0, WeightSource::Scale, 20, MOISTURE),
        Err(Ok(Error::InvalidWeight)),
        "zero weight"
    );
    assert_eq!(
        mint("DEP-B0", 2_000, WeightSource::BagEstimate, 0, MOISTURE),
        Err(Ok(Error::InvalidWeight)),
        "bag estimate with no bags"
    );
    assert_eq!(
        mint("DEP-M", 2_000, WeightSource::Scale, 20, 10_001),
        Err(Ok(Error::InvalidMoisture)),
        "moisture above 100.00%"
    );

    // The boundary itself is accepted: 100.00% is a reading, not an error.
    assert!(mint("DEP-M-MAX", 2_000, WeightSource::Scale, 20, 10_000).is_ok());

    // A scale-weighed lot with no bag count is legitimate (bulk intake).
    assert!(mint("DEP-BULK", 2_000, WeightSource::Scale, 0, MOISTURE).is_ok());
}

#[test]
fn mint_emits_a_minted_event() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-EVT");

    let expected = crate::events::Minted {
        receipt_id: id,
        warehouse_id: f.warehouse.clone(),
        deposit_id: String::from_str(&env, "DEP-EVT"),
        owner: f.farmer.clone(),
        commodity: soroban_sdk::Symbol::new(&env, super::COMMODITY),
        grade: soroban_sdk::Symbol::new(&env, super::GRADE),
        weight_kg: 2_000,
        weight_source: crate::WeightSource::Scale,
        bag_count: 20,
        moisture_pct: super::MOISTURE,
        custodian: f.custodian.clone(),
        verifier: f.verifier.clone(),
        deposit_date: 0,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &f.contract_id)]
    );
}
