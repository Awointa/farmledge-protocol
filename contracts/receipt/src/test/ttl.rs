use soroban_sdk::{
    testutils::{storage::Persistent as _, Ledger as _},
    Env,
};

use super::setup;
use crate::storage::{DataKey, DAY_IN_LEDGERS, ENTRY_TTL_EXTEND_TO, ENTRY_TTL_THRESHOLD};
use crate::Error;

fn receipt_ttl(env: &Env, f: &super::Fixture, receipt_id: u64) -> u32 {
    env.as_contract(&f.contract_id, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::Receipt(receipt_id))
    })
}

#[test]
fn minting_gives_a_receipt_a_full_ttl() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    assert_eq!(receipt_ttl(&env, &f, id), ENTRY_TTL_EXTEND_TO);
}

/// A receipt is expected to live 6-12 months, but the network caps any single
/// bump at 180 days. This is the test that pins that reality down: let a
/// receipt age past the bump threshold, then prove `extend_ttl` restores it.
#[test]
fn extend_ttl_restores_an_ageing_receipt() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    // Five months on.
    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);

    let aged = receipt_ttl(&env, &f, id);
    assert_eq!(aged, ENTRY_TTL_EXTEND_TO - 150 * DAY_IN_LEDGERS);
    assert!(
        aged < ENTRY_TTL_THRESHOLD,
        "must be below the bump threshold for the bump to do anything"
    );

    f.client.extend_ttl(&id);

    assert_eq!(receipt_ttl(&env, &f, id), ENTRY_TTL_EXTEND_TO);
}

/// No authorisation at all: a keep-alive job runs unprivileged, and so can
/// anyone else with an interest in the receipt surviving.
#[test]
fn anyone_can_extend_ttl_without_authorisation() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    // Not mock_all_auths, not mock_auths — no signature is available to anyone.
    env.set_auths(&[]);

    f.client.extend_ttl(&id);
    assert_eq!(receipt_ttl(&env, &f, id), ENTRY_TTL_EXTEND_TO);
}

/// The duplicate-mint guard has to outlive the receipt it protects, otherwise
/// an archived `deposit_id` entry would let the same intake be minted twice.
#[test]
fn extend_ttl_also_refreshes_the_deposit_id_guard() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");
    let deposit_id = soroban_sdk::String::from_str(&env, "DEP-0001");

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    f.client.extend_ttl(&id);

    let guard_ttl = env.as_contract(&f.contract_id, || {
        env.storage()
            .persistent()
            .get_ttl(&DataKey::DepositIndex(deposit_id))
    });
    assert_eq!(guard_ttl, ENTRY_TTL_EXTEND_TO);
}

#[test]
fn extend_ttl_rejects_an_unknown_receipt() {
    let env = Env::default();
    let f = setup(&env);

    assert_eq!(
        f.client.try_extend_ttl(&999),
        Err(Ok(Error::ReceiptNotFound))
    );
}

#[test]
fn extend_ttl_emits_an_event() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-EVT");

    f.client.extend_ttl(&id);

    let expected = crate::events::TtlExtended { receipt_id: id };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &f.contract_id)]
    );
}
