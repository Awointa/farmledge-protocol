//! Paged reads the backend indexer and lender portal rely on, and the data
//! version stamped on every stored record.

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

use super::{setup, setup_lending, Lending, DUE, GRACE, NOW};
use crate::DATA_VERSION;

/// Mints, locks to `lender`, defaults and sells one receipt. Returns the
/// `sale_id`.
fn sell(env: &Env, l: &Lending, lender: &Address, deposit: &str) -> u64 {
    env.ledger().set_timestamp(NOW);
    let id = l.f.mint(env, deposit);
    l.f.client.lock(
        &id,
        lender,
        &2_000,
        &String::from_str(env, "LOAN"),
        &DUE,
        &GRACE,
    );
    env.ledger().set_timestamp(DUE + GRACE + 1);
    l.f.client.start_recovery(&id);
    l.f.client.complete_recovery(
        &id,
        &l.operator,
        &Address::generate(env),
        &String::from_str(env, "SALE"),
    )
}

#[test]
fn sales_are_listed_per_lender_settled_or_not() {
    let env = Env::default();
    let l = setup_lending(&env);
    let other = Address::generate(&env);

    let a = sell(&env, &l, &l.lender, "DEP-A");
    let b = sell(&env, &l, &other, "DEP-B");
    let c = sell(&env, &l, &l.lender, "DEP-C");
    l.f.client
        .record_settlement(&a, &l.operator, &String::from_str(&env, "PAY-A"));

    let mine: std::vec::Vec<_> =
        l.f.client
            .get_sales_by_lender(&l.lender, &0)
            .iter()
            .collect();
    assert_eq!(
        mine.iter().map(|s| s.sale_id).collect::<std::vec::Vec<_>>(),
        std::vec![a, c]
    );
    assert_eq!(
        mine[0].settlement_ref,
        Some(String::from_str(&env, "PAY-A"))
    );
    assert_eq!(mine[1].settlement_ref, None);
    assert_eq!(l.f.client.lender_sale_page_count(&l.lender), 1);

    let theirs: std::vec::Vec<u64> =
        l.f.client
            .get_sales_by_lender(&other, &0)
            .iter()
            .map(|s| s.sale_id)
            .collect();
    assert_eq!(theirs, std::vec![b]);

    let stranger = Address::generate(&env);
    assert_eq!(l.f.client.lender_sale_page_count(&stranger), 0);
    assert!(l.f.client.get_sales_by_lender(&stranger, &0).is_empty());
    assert!(l.f.client.get_sales_by_lender(&l.lender, &1).is_empty());
}

#[test]
fn lender_sale_pages_roll_over_at_64() {
    let env = Env::default();
    let l = setup_lending(&env);
    let page = crate::storage::INDEX_PAGE_SIZE;
    let mut sold = std::vec::Vec::new();
    for i in 0..=page {
        sold.push(sell(&env, &l, &l.lender, &std::format!("DEP-{i}")));
    }

    assert_eq!(l.f.client.lender_sale_page_count(&l.lender), 2);
    let first: std::vec::Vec<u64> =
        l.f.client
            .get_sales_by_lender(&l.lender, &0)
            .iter()
            .map(|s| s.sale_id)
            .collect();
    assert_eq!(first, sold[..page as usize]);
    let second: std::vec::Vec<u64> =
        l.f.client
            .get_sales_by_lender(&l.lender, &1)
            .iter()
            .map(|s| s.sale_id)
            .collect();
    assert_eq!(second, sold[page as usize..]);
}

#[test]
fn every_record_written_carries_the_current_data_version() {
    let env = Env::default();
    let l = setup_lending(&env);
    assert_eq!(l.f.client.data_version(), DATA_VERSION);

    let root = l.f.mint(&env, "DEP-1");
    let locked = l.lock(&env, root, 1_500);
    l.default_and_recover(&env, locked);
    let sale = l.complete(&env, locked, &Address::generate(&env));

    for id in [root, locked] {
        assert_eq!(l.f.client.get_receipt(&id).data_version, DATA_VERSION);
    }
    assert_eq!(l.f.client.get_sale(&sale).data_version, DATA_VERSION);
}

#[test]
fn owner_and_warehouse_reads_are_paged_and_bounded() {
    let env = Env::default();
    let f = setup(&env);
    for i in 0..70 {
        f.mint_weighing(&env, &std::format!("DEP-{i}"), 100, 1);
    }
    let page = crate::storage::INDEX_PAGE_SIZE;
    assert_eq!(f.client.owner_page_count(&f.farmer), 2);
    assert_eq!(f.client.warehouse_page_count(&f.warehouse), 2);
    assert_eq!(f.client.get_receipts_by_owner(&f.farmer, &0).len(), page);
    assert_eq!(
        f.client.get_receipts_by_owner(&f.farmer, &1).len(),
        70 - page
    );
    assert_eq!(
        f.client.get_receipts_by_warehouse(&f.warehouse, &1).len(),
        70 - page
    );
    assert!(f.client.get_receipts_by_owner(&f.farmer, &2).is_empty());
}
