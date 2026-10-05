use soroban_sdk::{
    testutils::{
        storage::{Instance as _, Persistent as _},
        Address as _, Ledger as _,
    },
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

    let expected = crate::events::TtlExtended {
        receipt_id: id,
        warehouse_id: f.warehouse.clone(),
        owner: f.farmer.clone(),
        status: crate::Status::Active,
        weight_kg: 2_000,
        extended_at: env.ledger().timestamp(),
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &f.contract_id)]
    );
}

// --- batched keep-alive ----------------------------------------------------

fn ttl_of(env: &Env, f: &super::Fixture, key: &DataKey) -> u32 {
    env.as_contract(&f.contract_id, || env.storage().persistent().get_ttl(key))
}

fn ids(env: &Env, ids: &[u64]) -> soroban_sdk::Vec<u64> {
    soroban_sdk::Vec::from_slice(env, ids)
}

#[test]
fn extend_ttl_batch_refreshes_every_receipt_named() {
    let env = Env::default();
    let f = setup(&env);
    let a = f.mint(&env, "DEP-A");
    let b = f.mint(&env, "DEP-B");
    let c = f.mint(&env, "DEP-C");

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    assert_eq!(f.client.extend_ttl_batch(&ids(&env, &[a, c])), 2);

    assert_eq!(receipt_ttl(&env, &f, a), ENTRY_TTL_EXTEND_TO);
    assert_eq!(receipt_ttl(&env, &f, c), ENTRY_TTL_EXTEND_TO);
    // Only what was named.
    assert!(receipt_ttl(&env, &f, b) < ENTRY_TTL_THRESHOLD);
}

#[test]
fn extend_ttl_batch_needs_no_authorisation() {
    let env = Env::default();
    let f = setup(&env);
    let a = f.mint(&env, "DEP-A");
    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);

    env.mock_auths(&[]);
    assert_eq!(f.client.extend_ttl_batch(&ids(&env, &[a])), 1);
    assert_eq!(receipt_ttl(&env, &f, a), ENTRY_TTL_EXTEND_TO);
}

#[test]
fn extend_ttl_batch_is_bounded() {
    let env = Env::default();
    let f = setup(&env);
    let a = f.mint(&env, "DEP-A");

    let at_limit: std::vec::Vec<u64> = (0..crate::MAX_TTL_BATCH).map(|_| a).collect();
    assert_eq!(
        f.client.extend_ttl_batch(&ids(&env, &at_limit)),
        crate::MAX_TTL_BATCH
    );

    let over: std::vec::Vec<u64> = (0..=crate::MAX_TTL_BATCH).map(|_| a).collect();
    assert_eq!(
        f.client.try_extend_ttl_batch(&ids(&env, &over)),
        Err(Ok(Error::BatchTooLarge))
    );
}

/// An unknown id means the caller's list has drifted from the chain. The
/// whole batch fails, so nothing in it is silently skipped.
#[test]
fn extend_ttl_batch_with_an_unknown_id_extends_nothing() {
    let env = Env::default();
    let f = setup(&env);
    let a = f.mint(&env, "DEP-A");
    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    let aged = receipt_ttl(&env, &f, a);

    assert_eq!(
        f.client.try_extend_ttl_batch(&ids(&env, &[a, 999])),
        Err(Ok(Error::ReceiptNotFound))
    );
    assert_eq!(receipt_ttl(&env, &f, a), aged);
}

#[test]
fn extend_ttl_batch_emits_one_self_contained_event_per_receipt() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let a = f.mint(&env, "DEP-A");
    let b = f.mint_weighing(&env, "DEP-B", 700, 7);

    f.client.extend_ttl_batch(&ids(&env, &[a, b]));

    let event = |id, kg| crate::events::TtlExtended {
        receipt_id: id,
        warehouse_id: f.warehouse.clone(),
        owner: f.farmer.clone(),
        status: crate::Status::Active,
        weight_kg: kg,
        extended_at: env.ledger().timestamp(),
    };
    assert_eq!(
        env.events().all(),
        std::vec![
            event(a, 2_000).to_xdr(&env, &f.contract_id),
            event(b, 700).to_xdr(&env, &f.contract_id),
        ]
    );
}

// --- index pages -----------------------------------------------------------

/// The archival gap `extend_index_ttl` closes: a full index page is never
/// written again, so ordinary traffic does not keep it alive.
#[test]
fn a_full_index_page_ages_even_while_the_index_is_in_use() {
    let env = Env::default();
    let f = setup(&env);
    for i in 0..crate::storage::INDEX_PAGE_SIZE {
        f.mint(&env, &std::format!("DEP-{i}"));
    }
    let page0 = DataKey::OwnerIndex(f.farmer.clone(), 0);

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    // A new mint lands on page 1 and leaves page 0 untouched.
    f.mint(&env, "DEP-NEXT");
    assert!(ttl_of(&env, &f, &page0) < ENTRY_TTL_THRESHOLD);

    let extended = f
        .client
        .extend_index_ttl(&crate::Index::Owner(f.farmer.clone()), &0, &5);
    assert_eq!(extended, 2, "pages 0 and 1 exist; 2..5 are skipped");
    assert_eq!(ttl_of(&env, &f, &page0), ENTRY_TTL_EXTEND_TO);
    assert_eq!(
        ttl_of(&env, &f, &DataKey::OwnerPages(f.farmer.clone())),
        ENTRY_TTL_EXTEND_TO
    );
}

#[test]
fn extend_index_ttl_covers_every_index_kind_and_is_bounded() {
    let env = Env::default();
    let l = super::setup_lending(&env);
    let id = l.f.mint(&env, "DEP-1");
    l.lock(&env, id, 2_000);
    l.default_and_recover(&env, id);
    l.complete(&env, id, &soroban_sdk::Address::generate(&env));

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    env.mock_auths(&[]);
    let cases = [
        (
            crate::Index::Owner(l.f.farmer.clone()),
            DataKey::OwnerIndex(l.f.farmer.clone(), 0),
        ),
        (
            crate::Index::Warehouse(l.f.warehouse.clone()),
            DataKey::WarehouseIndex(l.f.warehouse.clone(), 0),
        ),
        (
            crate::Index::Lender(l.lender.clone()),
            DataKey::LenderIndex(l.lender.clone(), 0),
        ),
        (
            crate::Index::LenderSales(l.lender.clone()),
            DataKey::LenderSaleIndex(l.lender.clone(), 0),
        ),
    ];
    for (index, page0) in cases {
        assert!(ttl_of(&env, &l.f, &page0) < ENTRY_TTL_THRESHOLD);
        assert_eq!(l.f.client.extend_index_ttl(&index, &0, &1), 1);
        assert_eq!(ttl_of(&env, &l.f, &page0), ENTRY_TTL_EXTEND_TO);
    }

    assert_eq!(
        l.f.client.try_extend_index_ttl(
            &crate::Index::Owner(l.f.farmer.clone()),
            &0,
            &(crate::MAX_TTL_BATCH + 1)
        ),
        Err(Ok(Error::BatchTooLarge))
    );
    // The bound is inclusive: a full-size request is accepted.
    assert_eq!(
        l.f.client.extend_index_ttl(
            &crate::Index::Owner(l.f.farmer.clone()),
            &0,
            &crate::MAX_TTL_BATCH
        ),
        1
    );
    // An index nobody has written is not an error: there is nothing to keep.
    assert_eq!(
        l.f.client.extend_index_ttl(
            &crate::Index::Owner(soroban_sdk::Address::generate(&env)),
            &0,
            &3
        ),
        0
    );
}

// --- what archival actually does -------------------------------------------

/// Persistent entries are never deleted. Since protocol 23 an archived one is
/// restored automatically when a transaction touches it — simulation records
/// it as a restore and the caller pays for that — and comes back with the
/// network's minimum TTL. The test host behaves the same way in its
/// recording mode, which is what this pins: the receipt reads back intact,
/// but with only the minimum TTL, so the keep-alive job must still extend it.
#[test]
fn an_archived_receipt_is_restored_intact_and_needs_extending_again() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-A");
    let other = f.mint(&env, "DEP-B");
    let before = f.client.get_receipt(&id);

    // Keep the instance (and `other`) alive past the point `id` expires.
    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    f.client.extend_ttl(&other);
    env.ledger()
        .set_sequence_number(ENTRY_TTL_EXTEND_TO + DAY_IN_LEDGERS);

    assert_eq!(f.client.get_receipt(&id), before);
    let min_ttl = env.ledger().get().min_persistent_entry_ttl;
    assert_eq!(receipt_ttl(&env, &f, id), min_ttl - 1);
    assert!(receipt_ttl(&env, &f, id) < ENTRY_TTL_THRESHOLD);

    f.client.extend_ttl(&id);
    assert_eq!(receipt_ttl(&env, &f, id), ENTRY_TTL_EXTEND_TO);
}

// --- the contract instance -------------------------------------------------

fn instance_ttl(env: &Env, f: &super::Fixture) -> u32 {
    env.as_contract(&f.contract_id, || env.storage().instance().get_ttl())
}

/// The instance holds the admin and the id counters, and its TTL also covers
/// the contract's code. If it lapsed, every call would need a restore first.
/// A keep-alive call must refresh it even when nothing else is written.
#[test]
fn keep_alive_calls_refresh_the_contract_instance() {
    use crate::storage::{INSTANCE_TTL_EXTEND_TO, INSTANCE_TTL_THRESHOLD};

    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-A");
    assert_eq!(instance_ttl(&env, &f), INSTANCE_TTL_EXTEND_TO);

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    assert!(instance_ttl(&env, &f) < INSTANCE_TTL_THRESHOLD);
    f.client.extend_ttl_batch(&ids(&env, &[id]));
    assert_eq!(instance_ttl(&env, &f), INSTANCE_TTL_EXTEND_TO);

    env.ledger().set_sequence_number(300 * DAY_IN_LEDGERS);
    assert!(instance_ttl(&env, &f) < INSTANCE_TTL_THRESHOLD);
    f.client
        .extend_index_ttl(&crate::Index::Owner(f.farmer.clone()), &0, &1);
    assert_eq!(instance_ttl(&env, &f), INSTANCE_TTL_EXTEND_TO);
}

/// Ordinary state changes refresh it too, so an active deployment never
/// depends on the keep-alive job for its own instance.
#[test]
fn state_changes_refresh_the_contract_instance() {
    use crate::storage::INSTANCE_TTL_EXTEND_TO;

    let env = Env::default();
    let f = setup(&env);
    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    f.mint(&env, "DEP-A");
    assert_eq!(instance_ttl(&env, &f), INSTANCE_TTL_EXTEND_TO);
}

// --- finished receipts are left to archive --------------------------------

/// `Exited` and `Revoked` receipts can never be acted on again, so the
/// keep-alive skips them: no bump for the receipt or its deposit guard, no
/// event, and they do not count as extended.
#[test]
fn extend_ttl_batch_skips_exited_and_revoked_receipts() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let live = f.mint(&env, "DEP-LIVE");
    let exited = f.mint(&env, "DEP-EXITED");
    let revoked = f.mint(&env, "DEP-REVOKED");
    f.client.exit(&exited, &f.custodian, &2_000, &2_000);
    f.client
        .revoke(&revoked, &soroban_sdk::String::from_str(&env, "fraud"));

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    assert_eq!(
        f.client
            .extend_ttl_batch(&ids(&env, &[live, exited, revoked])),
        1
    );
    let only_live = crate::events::TtlExtended {
        receipt_id: live,
        warehouse_id: f.warehouse.clone(),
        owner: f.farmer.clone(),
        status: crate::Status::Active,
        weight_kg: 2_000,
        extended_at: env.ledger().timestamp(),
    };
    assert_eq!(
        env.events().all(),
        std::vec![only_live.to_xdr(&env, &f.contract_id)],
        "one event, for the live receipt"
    );

    assert_eq!(receipt_ttl(&env, &f, live), ENTRY_TTL_EXTEND_TO);
    for id in [exited, revoked] {
        assert!(receipt_ttl(&env, &f, id) < ENTRY_TTL_THRESHOLD);
    }
}

/// After a partial exit the receipt that kept the rest is still live and
/// still kept alive.
#[test]
fn after_a_partial_exit_only_the_exited_part_is_left_to_archive() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-PART");
    let gone = f.client.exit(&id, &f.custodian, &500, &500);

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    assert_eq!(f.client.extend_ttl_batch(&ids(&env, &[id, gone])), 1);
    assert_eq!(receipt_ttl(&env, &f, id), ENTRY_TTL_EXTEND_TO);
    assert!(receipt_ttl(&env, &f, gone) < ENTRY_TTL_THRESHOLD);
}

/// Nothing keeps the claim bitmap alive, so a chunk whose lots have all
/// exited ages out. That must not reopen its deposit numbers: an archived
/// chunk is restored when the duplicate mint reads it, and the mint is
/// still refused (audit finding A3). The archived receipt reads back intact.
#[test]
fn an_archived_claim_chunk_still_blocks_a_duplicate_mint() {
    let env = Env::default();
    let f = setup(&env);
    let mut day = [f.balance_leaf(&env, "DEP-DONE", 2_000, 20, crate::WeightSource::Scale)];
    let proofs = f.anchor_next_day(&env, &mut day);
    let id = f
        .client
        .mint_from_anchor(&f.custodian, &f.verifier, &day[0], &proofs[0]);
    f.client.exit(&id, &f.custodian, &2_000, &1_990);
    let before = f.client.get_receipt(&id);

    let chunk = DataKey::Claimed(f.warehouse.clone(), day[0].deposit_no / 1024);
    let live_until = env.ledger().sequence() + ttl_of(&env, &f, &chunk);
    // Reading a TTL restores the entry, so archival is shown by arithmetic:
    // the ledger moves past the chunk's last live ledger.
    env.ledger()
        .set_sequence_number(ENTRY_TTL_EXTEND_TO + DAY_IN_LEDGERS);
    assert!(
        env.ledger().sequence() > live_until,
        "the chunk has archived"
    );

    // The same lot again, in a fresh day's books.
    let mut again = [day[0].clone()];
    let proofs = f.anchor_next_day(&env, &mut again);
    assert_eq!(
        f.client
            .try_mint_from_anchor(&f.custodian, &f.verifier, &again[0], &proofs[0]),
        Err(Ok(Error::DuplicateDepositId))
    );
    assert_eq!(f.client.get_receipt(&id), before);
}
