use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, Symbol,
};

use super::{setup, setup_bare, WAREHOUSE};
use crate::{Error, ReceiptContract, ReceiptContractClient, WeightSource};

#[test]
fn the_constructor_installs_the_admin_in_the_deploy_itself() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let stranger = Address::generate(&env);

    // Registered with its admin in one step: there is no moment at which the
    // contract exists without one, and so nothing for a third party to claim.
    let contract_id = env.register(ReceiptContract, (admin.clone(),));
    let client = ReceiptContractClient::new(&env, &contract_id);
    let warehouse = Symbol::new(&env, WAREHOUSE);

    let add_as = |who: &Address| {
        env.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke {
                contract: &contract_id,
                fn_name: "add_warehouse",
                args: (warehouse.clone(),).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        client.try_add_warehouse(&warehouse)
    };
    assert!(add_as(&stranger).is_err());
    assert_eq!(add_as(&admin), Ok(Ok(())));
}

// --- happy paths -----------------------------------------------------------

#[test]
fn admin_manages_warehouses_and_custodians() {
    let env = Env::default();
    let f = setup_bare(&env);
    let warehouse = Symbol::new(&env, WAREHOUSE);

    // A custodian cannot be attached to a warehouse that does not exist.
    assert_eq!(
        f.client.try_add_custodian(&warehouse, &f.custodian),
        Err(Ok(Error::WarehouseNotRegistered))
    );

    f.client.add_warehouse(&warehouse);
    assert_eq!(
        f.client.try_add_warehouse(&warehouse),
        Err(Ok(Error::AlreadyRegistered))
    );

    f.client.add_custodian(&warehouse, &f.custodian);
    assert_eq!(
        f.client.try_add_custodian(&warehouse, &f.custodian),
        Err(Ok(Error::AlreadyRegistered))
    );

    f.client.remove_custodian(&warehouse, &f.custodian);
    assert_eq!(
        f.client.try_remove_custodian(&warehouse, &f.custodian),
        Err(Ok(Error::NotRegistered))
    );

    f.client.remove_warehouse(&warehouse);
    assert_eq!(
        f.client.try_remove_warehouse(&warehouse),
        Err(Ok(Error::NotRegistered))
    );
}

#[test]
fn admin_manages_verifiers_and_recovery_operators() {
    let env = Env::default();
    let f = setup_bare(&env);
    let verifier = Address::generate(&env);
    let operator = Address::generate(&env);

    f.client.add_verifier(&verifier);
    assert_eq!(
        f.client.try_add_verifier(&verifier),
        Err(Ok(Error::AlreadyRegistered))
    );
    f.client.remove_verifier(&verifier);
    assert_eq!(
        f.client.try_remove_verifier(&verifier),
        Err(Ok(Error::NotRegistered))
    );

    // Recovery operators are stored now and read by complete_recovery next
    // session; nothing on the public surface consumes them yet, so this
    // asserts through the internal predicate the recovery flow will use.
    f.client.add_recovery_operator(&operator);
    assert!(env.as_contract(&f.contract_id, || {
        crate::admin::is_recovery_operator(&env, &operator)
    }));

    f.client.remove_recovery_operator(&operator);
    assert!(!env.as_contract(&f.contract_id, || {
        crate::admin::is_recovery_operator(&env, &operator)
    }));
    assert_eq!(
        f.client.try_remove_recovery_operator(&operator),
        Err(Ok(Error::NotRegistered))
    );
}

// --- authorisation ---------------------------------------------------------

/// Every registry entry point, checked against a non-admin signer.
///
/// The admin is loaded from storage and authorised there, so signing as
/// yourself proves nothing — this is the test that would fail if any of these
/// ever started authorising a caller-supplied address instead.
#[test]
fn a_non_admin_cannot_touch_any_registry() {
    let env = Env::default();
    let f = setup_bare(&env);

    let intruder = Address::generate(&env);
    let warehouse = Symbol::new(&env, WAREHOUSE);
    let subject = Address::generate(&env);

    // Pre-register everything as admin so the "remove" calls below would
    // otherwise succeed — the only thing stopping them is authorisation.
    f.client.add_warehouse(&warehouse);
    f.client.add_custodian(&warehouse, &subject);
    f.client.add_verifier(&subject);
    f.client.add_recovery_operator(&subject);

    /// Scope the only available signature to the intruder for one call.
    macro_rules! as_intruder {
        ($fn_name:literal, $args:expr, $call:expr) => {{
            env.mock_auths(&[MockAuth {
                address: &intruder,
                invoke: &MockAuthInvoke {
                    contract: &f.contract_id,
                    fn_name: $fn_name,
                    args: $args,
                    sub_invokes: &[],
                },
            }]);
            assert!($call.is_err(), "{} should be admin-only", $fn_name);
        }};
    }

    let other = Symbol::new(&env, "WH_OTHER_99");
    as_intruder!(
        "add_warehouse",
        (other.clone(),).into_val(&env),
        f.client.try_add_warehouse(&other)
    );
    as_intruder!(
        "remove_warehouse",
        (warehouse.clone(),).into_val(&env),
        f.client.try_remove_warehouse(&warehouse)
    );
    as_intruder!(
        "add_custodian",
        (warehouse.clone(), intruder.clone()).into_val(&env),
        f.client.try_add_custodian(&warehouse, &intruder)
    );
    as_intruder!(
        "remove_custodian",
        (warehouse.clone(), subject.clone()).into_val(&env),
        f.client.try_remove_custodian(&warehouse, &subject)
    );
    as_intruder!(
        "add_verifier",
        (intruder.clone(),).into_val(&env),
        f.client.try_add_verifier(&intruder)
    );
    as_intruder!(
        "remove_verifier",
        (subject.clone(),).into_val(&env),
        f.client.try_remove_verifier(&subject)
    );
    as_intruder!(
        "add_recovery_operator",
        (intruder.clone(),).into_val(&env),
        f.client.try_add_recovery_operator(&intruder)
    );
    as_intruder!(
        "remove_recovery_operator",
        (subject.clone(),).into_val(&env),
        f.client.try_remove_recovery_operator(&subject)
    );

    // Nothing moved.
    env.mock_all_auths();
    assert_eq!(
        f.client.try_add_custodian(&warehouse, &subject),
        Err(Ok(Error::AlreadyRegistered)),
        "the intruder's remove_custodian must not have gone through"
    );
    assert_eq!(
        f.client.try_add_warehouse(&warehouse),
        Err(Ok(Error::AlreadyRegistered)),
        "the intruder's remove_warehouse must not have gone through"
    );
}

// --- registries actually gate minting --------------------------------------

#[test]
fn deregistering_a_custodian_stops_them_minting_and_anchoring() {
    let env = Env::default();
    let f = setup(&env);

    f.mint(&env, "DEP-BEFORE");
    let mut day = [f.balance_leaf(&env, "DEP-AFTER", 2_000, 20, WeightSource::Scale)];
    let proofs = f.anchor_next_day(&env, &mut day);
    f.client.remove_custodian(&f.warehouse, &f.custodian);

    assert_eq!(
        f.client
            .try_mint_from_anchor(&f.custodian, &f.verifier, &day[0], &proofs[0]),
        Err(Ok(Error::CustodianNotRegistered))
    );
    assert_eq!(
        f.client.try_anchor(
            &f.custodian,
            &f.warehouse,
            &f.next_day(),
            &f.client.leaf_hash(&day[0]),
            &1
        ),
        Err(Ok(Error::CustodianNotRegistered))
    );
}

#[test]
fn deregistering_a_verifier_stops_them_minting() {
    let env = Env::default();
    let f = setup(&env);

    let mut day = [f.balance_leaf(&env, "DEP-AFTER", 2_000, 20, WeightSource::Scale)];
    let proofs = f.anchor_next_day(&env, &mut day);
    f.client.remove_verifier(&f.verifier);

    assert_eq!(
        f.client
            .try_mint_from_anchor(&f.custodian, &f.verifier, &day[0], &proofs[0]),
        Err(Ok(Error::VerifierNotRegistered))
    );
}

#[test]
fn deregistering_a_warehouse_stops_minting_against_it() {
    let env = Env::default();
    let f = setup(&env);

    f.mint(&env, "DEP-BEFORE");
    let mut day = [f.balance_leaf(&env, "DEP-AFTER", 2_000, 20, WeightSource::Scale)];
    let proofs = f.anchor_next_day(&env, &mut day);
    f.client.remove_warehouse(&f.warehouse);

    assert_eq!(
        f.client
            .try_mint_from_anchor(&f.custodian, &f.verifier, &day[0], &proofs[0]),
        Err(Ok(Error::WarehouseNotRegistered)),
        "an orphaned custodian registration must not keep the warehouse alive"
    );

    // Receipts already issued stay readable, and — deliberately — still
    // exitable: ending a warehouse partnership must not strand a farmer's
    // grain inside it. `exit` checks the custodian, not the warehouse.
    assert!(f.client.try_get_receipt(&1).is_ok());
    f.client.exit(&1, &f.custodian, &2_000, &2_000);
    assert_eq!(f.client.get_receipt(&1).status, crate::Status::Exited);
}

#[test]
fn every_registry_change_emits_its_event_with_a_timestamp() {
    use crate::events::*;
    use soroban_sdk::{
        testutils::{Events as _, Ledger as _},
        Event as _,
    };

    let env = Env::default();
    let f = setup_bare(&env);
    env.ledger().set_timestamp(1_700_000_123);
    let at = 1_700_000_123;
    let wh = f.warehouse.clone();
    let operator = Address::generate(&env);

    // `env.events().all()` holds the events of the most recent call only.
    let check = |expected: soroban_sdk::xdr::ContractEvent| {
        assert_eq!(env.events().all(), std::vec![expected]);
    };
    let id = &f.contract_id;

    f.client.add_warehouse(&wh);
    check(
        WarehouseAdded {
            warehouse_id: wh.clone(),
            added_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.add_custodian(&wh, &f.custodian);
    check(
        CustodianAdded {
            warehouse_id: wh.clone(),
            custodian: f.custodian.clone(),
            added_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.add_verifier(&f.verifier);
    check(
        VerifierAdded {
            verifier: f.verifier.clone(),
            added_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.add_recovery_operator(&operator);
    check(
        RecoveryOperatorAdded {
            operator: operator.clone(),
            added_at: at,
        }
        .to_xdr(&env, id),
    );

    f.client.remove_recovery_operator(&operator);
    check(
        RecoveryOperatorRemoved {
            operator,
            removed_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.remove_verifier(&f.verifier);
    check(
        VerifierRemoved {
            verifier: f.verifier.clone(),
            removed_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.remove_custodian(&wh, &f.custodian);
    check(
        CustodianRemoved {
            warehouse_id: wh.clone(),
            custodian: f.custodian.clone(),
            removed_at: at,
        }
        .to_xdr(&env, id),
    );
    f.client.remove_warehouse(&wh);
    check(
        WarehouseRemoved {
            warehouse_id: wh,
            removed_at: at,
        }
        .to_xdr(&env, id),
    );
}
