use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, IntoVal, String,
};

use super::{setup_lending, sign_only, Lending, DUE, GRACE, LOAN_REF};
use crate::{Error, RecoveryResolution, Status};

const SETTLED_AT: u64 = DUE + GRACE + 5_000;

/// Mints, locks, defaults and sells a 2,000kg receipt. Returns
/// `(receipt_id, sale_id, buyer)`.
fn sold(env: &Env, l: &Lending, deposit: &str) -> (u64, u64, Address) {
    let id = l.f.mint(env, deposit);
    l.lock(env, id, 2_000);
    l.default_and_recover(env, id);
    let buyer = Address::generate(env);
    let sale_id = l.complete(env, id, &buyer);
    (id, sale_id, buyer)
}

fn try_settle(
    env: &Env,
    l: &Lending,
    sale_id: u64,
    operator: &Address,
    reference: &str,
) -> Result<(), Result<Error, soroban_sdk::InvokeError>> {
    l.f.client
        .try_record_settlement(&sale_id, operator, &String::from_str(env, reference))
        .map(|r| r.unwrap())
}

fn settle_args(
    env: &Env,
    l: &Lending,
    sale_id: u64,
    reference: &str,
) -> soroban_sdk::Vec<soroban_sdk::Val> {
    (
        sale_id,
        l.operator.clone(),
        String::from_str(env, reference),
    )
        .into_val(env)
}

// --- authorisation ---------------------------------------------------------

#[test]
fn record_settlement_with_only_the_operator_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");

    let args = settle_args(&env, &l, sale_id, "SETTLE-1");
    sign_only(
        &env,
        &l.f.contract_id,
        "record_settlement",
        args,
        &[&l.operator],
    );
    assert!(try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1").is_err());
    assert_eq!(l.f.client.get_sale(&sale_id).settlement_ref, None);
}

#[test]
fn record_settlement_with_only_the_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");

    let args = settle_args(&env, &l, sale_id, "SETTLE-1");
    sign_only(
        &env,
        &l.f.contract_id,
        "record_settlement",
        args,
        &[&l.lender],
    );
    assert!(try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1").is_err());
    assert_eq!(l.f.client.get_sale(&sale_id).settlement_ref, None);
}

#[test]
fn record_settlement_with_the_operator_and_a_different_lender_fails() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");
    let other_lender = Address::generate(&env);

    let args = settle_args(&env, &l, sale_id, "SETTLE-1");
    sign_only(
        &env,
        &l.f.contract_id,
        "record_settlement",
        args,
        &[&l.operator, &other_lender],
    );
    assert!(try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1").is_err());
}

#[test]
fn record_settlement_with_the_operator_and_the_lender_succeeds() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");
    env.ledger().set_timestamp(SETTLED_AT);

    let args = settle_args(&env, &l, sale_id, "SETTLE-1");
    sign_only(
        &env,
        &l.f.contract_id,
        "record_settlement",
        args,
        &[&l.operator, &l.lender],
    );
    assert_eq!(
        try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1"),
        Ok(())
    );

    let sale = l.f.client.get_sale(&sale_id);
    assert_eq!(
        sale.settlement_ref,
        Some(String::from_str(&env, "SETTLE-1"))
    );
    assert_eq!(sale.settled_at, Some(SETTLED_AT));
}

#[test]
fn an_unregistered_operator_cannot_record_a_settlement() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");

    let impostor = Address::generate(&env);
    assert_eq!(
        try_settle(&env, &l, sale_id, &impostor, "SETTLE-1"),
        Err(Ok(Error::RecoveryOperatorNotRegistered))
    );
}

/// Any registered operator may settle, not only the one who ran the sale.
#[test]
fn a_different_registered_operator_may_record_the_settlement() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");

    let second = Address::generate(&env);
    l.f.client.add_recovery_operator(&second);
    assert_eq!(try_settle(&env, &l, sale_id, &second, "SETTLE-1"), Ok(()));
}

// --- state -----------------------------------------------------------------

#[test]
fn a_settlement_is_write_once() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, sale_id, _) = sold(&env, &l, "DEP-1");

    assert_eq!(
        try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1"),
        Ok(())
    );
    assert_eq!(
        try_settle(&env, &l, sale_id, &l.operator, "SETTLE-2"),
        Err(Ok(Error::RecoveryAlreadySettled))
    );
    // Even the identical reference is rejected: it is not a no-op, it is a
    // second write.
    assert_eq!(
        try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1"),
        Err(Ok(Error::RecoveryAlreadySettled))
    );
    assert_eq!(
        l.f.client.get_sale(&sale_id).settlement_ref,
        Some(String::from_str(&env, "SETTLE-1"))
    );
}

#[test]
fn record_settlement_is_rejected_for_a_recovery_that_did_not_complete() {
    let env = Env::default();
    let l = setup_lending(&env);

    // Open recovery: no sale exists yet.
    let open = l.f.mint(&env, "DEP-OPEN");
    l.lock(&env, open, 2_000);
    l.default_and_recover(&env, open);
    assert_eq!(l.f.client.get_receipt(&open).status, Status::Recovery);
    assert_eq!(
        try_settle(&env, &l, 1, &l.operator, "SETTLE-1"),
        Err(Ok(Error::SaleNotFound))
    );

    // Repaid recovery: resolved, but never sold.
    l.f.client.unlock(&open, &2_000);
    let history = l.f.client.get_receipt(&open).recovery_history;
    assert_eq!(
        history.get(0).unwrap().resolution,
        RecoveryResolution::Repaid
    );
    assert_eq!(history.get(0).unwrap().sale_id, None);
    assert_eq!(
        try_settle(&env, &l, 1, &l.operator, "SETTLE-1"),
        Err(Ok(Error::SaleNotFound))
    );
    assert_eq!(l.f.client.try_get_sale(&1), Err(Ok(Error::SaleNotFound)));
}

#[test]
fn unsettled_sales_are_listed_until_settled() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (_, first, _) = sold(&env, &l, "DEP-1");
    env.ledger().set_timestamp(super::NOW);
    let (_, second, _) = sold(&env, &l, "DEP-2");

    let pending = |l: &Lending| -> std::vec::Vec<u64> {
        l.f.client
            .get_unsettled_sales(&0)
            .iter()
            .map(|s| s.sale_id)
            .collect()
    };
    assert_eq!(pending(&l), std::vec![first, second]);
    assert_eq!(l.f.client.unsettled_sale_count(), 2);

    try_settle(&env, &l, first, &l.operator, "SETTLE-1").unwrap();
    assert_eq!(pending(&l), std::vec![second]);
    assert_eq!(l.f.client.unsettled_sale_count(), 1);

    try_settle(&env, &l, second, &l.operator, "SETTLE-2").unwrap();
    assert!(pending(&l).is_empty());
    assert!(l.f.client.get_unsettled_sales(&1).is_empty());
}

/// The reason settlement lives on the `Sale` rather than the receipt: the
/// buyer may split the receipt before the bank settles.
#[test]
fn a_settlement_recorded_after_the_buyer_splits_reaches_every_child() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (id, sale_id, _) = sold(&env, &l, "DEP-1");

    let exited = l.f.client.exit(&id, &l.f.custodian, &500, &495);
    let remainder = exited + 1;
    try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1").unwrap();

    for child in [exited, remainder] {
        let record =
            l.f.client
                .get_receipt(&child)
                .recovery_history
                .get(0)
                .unwrap();
        assert_eq!(record.sale_id, Some(sale_id));
        assert_eq!(
            l.f.client.get_sale(&sale_id).settlement_ref,
            Some(String::from_str(&env, "SETTLE-1"))
        );
    }
}

/// Selling the same receipt twice produces two sales, each settled on its
/// own — a receipt id is not a sale id.
#[test]
fn a_receipt_sold_twice_has_two_independently_settled_sales() {
    let env = Env::default();
    let l = setup_lending(&env);
    let (id, first, buyer) = sold(&env, &l, "DEP-1");
    try_settle(&env, &l, first, &l.operator, "SETTLE-1").unwrap();

    // The buyer pledges the whole receipt, defaults, and it is sold again.
    let now = DUE + GRACE + 10;
    env.ledger().set_timestamp(now);
    let lender2 = Address::generate(&env);
    l.f.client.lock(
        &id,
        &lender2,
        &2_000,
        &String::from_str(&env, "LOAN-2"),
        &(now + 100),
        &0,
    );
    env.ledger().set_timestamp(now + 101);
    l.f.client.start_recovery(&id);
    let buyer2 = Address::generate(&env);
    let second =
        l.f.client
            .complete_recovery(&id, &l.operator, &buyer2, &String::from_str(&env, "SALE-2"));

    assert_ne!(first, second);
    let second_sale = l.f.client.get_sale(&second);
    assert_eq!(second_sale.receipt_id, id);
    assert_eq!(second_sale.lender, lender2);
    assert_eq!(second_sale.previous_owner, buyer);
    assert_eq!(second_sale.settlement_ref, None);

    // The second sale's settlement needs the second lender, not the first.
    sign_only(
        &env,
        &l.f.contract_id,
        "record_settlement",
        (
            second,
            l.operator.clone(),
            String::from_str(&env, "SETTLE-2"),
        )
            .into_val(&env),
        &[&l.operator, &l.lender],
    );
    assert!(try_settle(&env, &l, second, &l.operator, "SETTLE-2").is_err());

    env.mock_all_auths();
    try_settle(&env, &l, second, &l.operator, "SETTLE-2").unwrap();
    assert_eq!(
        l.f.client.get_sale(&first).settlement_ref,
        Some(String::from_str(&env, "SETTLE-1")),
        "the first sale is untouched"
    );
    let history = l.f.client.get_receipt(&id).recovery_history;
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(1).unwrap().sale_id, Some(second));
}

#[test]
fn extend_ttl_keeps_the_sale_record_alive() {
    use crate::storage::{DataKey, DAY_IN_LEDGERS, ENTRY_TTL_EXTEND_TO};
    use soroban_sdk::testutils::storage::Persistent as _;

    let env = Env::default();
    let l = setup_lending(&env);
    let (id, sale_id, _) = sold(&env, &l, "DEP-1");

    env.ledger().set_sequence_number(150 * DAY_IN_LEDGERS);
    l.f.client.extend_ttl(&id);

    let ttl = env.as_contract(&l.f.contract_id, || {
        env.storage().persistent().get_ttl(&DataKey::Sale(sale_id))
    });
    assert_eq!(ttl, ENTRY_TTL_EXTEND_TO);
}

#[test]
fn settlement_recorded_event_carries_the_indexer_fields() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let l = setup_lending(&env);
    let (id, sale_id, buyer) = sold(&env, &l, "DEP-1");
    env.ledger().set_timestamp(SETTLED_AT);
    try_settle(&env, &l, sale_id, &l.operator, "SETTLE-1").unwrap();

    let expected = crate::events::SettlementRecorded {
        sale_id,
        lender: l.lender.clone(),
        receipt_id: id,
        operator: l.operator.clone(),
        previous_owner: l.f.farmer.clone(),
        buyer,
        warehouse_id: l.f.warehouse.clone(),
        loan_ref: String::from_str(&env, LOAN_REF),
        kg: 2_000,
        sale_ref: String::from_str(&env, "SALE-1"),
        settlement_ref: String::from_str(&env, "SETTLE-1"),
        settled_at: SETTLED_AT,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &l.f.contract_id)]
    );
}
