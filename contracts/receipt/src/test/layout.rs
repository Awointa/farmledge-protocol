//! The storage layout: positional, compact, and lossless.

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    xdr::ToXdr,
    Address, Env, String, Val, Vec,
};

use super::{setup, setup_lending};
use crate::layout::StoredReceipt;
use crate::storage::DataKey;
use crate::{Error, Status};

/// Bytes of a stored value, as the network serialises it.
fn stored_bytes(env: &Env, contract: &Address, key: &DataKey) -> u32 {
    env.as_contract(contract, || {
        let raw: Val = env.storage().persistent().get(key).unwrap();
        raw.to_xdr(env).len()
    })
}

#[test]
fn a_receipt_is_stored_without_field_names_or_its_own_id() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-1");

    let fields = env.as_contract(&f.contract_id, || {
        let raw: Vec<Val> = env
            .storage()
            .persistent()
            .get(&DataKey::Receipt(id))
            .unwrap();
        raw.len()
    });
    assert_eq!(fields, 20, "positional: one element per field, no id");
    let bytes = stored_bytes(&env, &f.contract_id, &DataKey::Receipt(id));
    // 316 bytes when written; it was 816 as a named-field map.
    assert!(
        bytes <= 330,
        "a fresh receipt's value grew to {bytes} bytes"
    );
}

#[test]
fn a_sale_is_stored_without_field_names_or_its_own_id() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.default_and_recover(&env, id);
    let sale = l.complete(&env, id, &Address::generate(&env));

    let fields = env.as_contract(&l.f.contract_id, || {
        let raw: Vec<Val> = env
            .storage()
            .persistent()
            .get(&DataKey::Sale(sale))
            .unwrap();
        raw.len()
    });
    assert_eq!(fields, 14);
    let bytes = stored_bytes(&env, &l.f.contract_id, &DataKey::Sale(sale));
    // 292 bytes when written.
    assert!(bytes <= 310, "a sale's value grew to {bytes} bytes");
}

/// Every field survives the round trip, including the lock, an open
/// recovery and a recovery history.
#[test]
fn the_layout_is_lossless() {
    let env = Env::default();
    let l = setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.default_and_recover(&env, id);
    l.complete(&env, id, &Address::generate(&env));
    let lender2 = Address::generate(&env);
    let now = env.ledger().timestamp();
    l.f.client.lock(
        &id,
        &lender2,
        &2_000,
        &String::from_str(&env, "LOAN-2"),
        &(now + 10),
        &5,
    );
    env.ledger().set_timestamp(now + 16);
    l.f.client.start_recovery(&id);

    let r = l.f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Recovery);
    assert_eq!(r.recovery_history.len(), 1);
    env.as_contract(&l.f.contract_id, || {
        let stored = crate::layout::store_receipt(&env, &r);
        assert_eq!(crate::layout::load_receipt(&env, id, stored), r);
    });

    let sale = l.f.client.get_sale(&1);
    assert_eq!(
        crate::layout::load_sale(1, crate::layout::store_sale(&sale)),
        sale
    );
}

/// A stored code no layout defines aborts with `CorruptRecord` rather than
/// being read as something it is not.
#[test]
fn an_unknown_stored_code_is_a_corrupt_record() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-1");

    env.as_contract(&f.contract_id, || {
        let key = DataKey::Receipt(id);
        let mut stored: StoredReceipt = env.storage().persistent().get(&key).unwrap();
        stored.13 = 99;
        env.storage().persistent().set(&key, &stored);
    });
    assert_eq!(f.client.try_get_receipt(&id), Err(Ok(Error::CorruptRecord)));
}
