use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, String, Symbol, Vec};

use super::{setup_lending, COMMODITY, GRADE, MOISTURE};
use crate::proto::Leaf;
use crate::{Error, WeightSource};

fn node(env: &Env, l: &BytesN<32>, r: &BytesN<32>) -> BytesN<32> {
    let mut b = soroban_sdk::Bytes::from_array(env, &[1u8]);
    b.append(&l.clone().into());
    b.append(&r.clone().into());
    env.crypto().sha256(&b).into()
}

#[test]
fn every_design_path_works_and_rejects_duplicates() {
    let env = Env::default();
    let l = setup_lending(&env);
    let f = &l.f;
    let sym = |s| Symbol::new(&env, s);

    // A
    let mint = |no: u64| {
        f.client.try_a_mint(&f.custodian, &f.verifier, &String::from_str(&env, "DEP-1"), &no,
            &f.farmer, &f.warehouse, &sym(COMMODITY), &sym(GRADE), &2_000, &WeightSource::Scale, &20, &MOISTURE)
    };
    assert!(mint(5).is_ok());
    assert_eq!(mint(5), Err(Ok(Error::DuplicateDepositId)));
    assert!(mint(5 + 1024).is_ok());

    // B + D: a 4-leaf tree
    let leaf = |i: u32| Leaf {
        version: 1, kind: 2, warehouse_id: f.warehouse.clone(), day: 20_000, index: i,
        deposit_id: String::from_str(&env, "DEP-9"), owner: f.farmer.clone(),
        commodity: sym(COMMODITY), grade: sym(GRADE), kg: 1_000 + i as u64, bags: 10,
        moisture_bps: MOISTURE, custodian: f.custodian.clone(), verifier: f.verifier.clone(),
        at: 0, salt: BytesN::from_array(&env, &[i as u8; 32]),
    };
    let h: std::vec::Vec<BytesN<32>> = (0..4).map(|i| f.client.leaf_hash(&leaf(i))).collect();
    let (n01, n23) = (node(&env, &h[0], &h[1]), node(&env, &h[2], &h[3]));
    let root = node(&env, &n01, &n23);
    f.client.b_anchor(&f.custodian, &f.warehouse, &20_000, &root, &4, &true);
    let proof2 = Vec::from_array(&env, [h[3].clone(), n01.clone()]);
    assert!(f.client.b_verify(&f.warehouse, &20_000, &h[2], &2, &proof2));
    assert!(!f.client.b_verify(&f.warehouse, &20_000, &h[1], &2, &proof2));

    let lender = Address::generate(&env);
    let pledge = |lf: &Leaf, p: &Vec<BytesN<32>>| f.client.try_d_pledge(&lender, lf, p,
        &String::from_str(&env, "LOAN"), &(10u64), &0);
    let id = pledge(&leaf(2), &proof2).unwrap().unwrap();
    assert_eq!(pledge(&leaf(2), &proof2), Err(Ok(Error::DuplicateDepositId)));
    let mut forged = leaf(2);
    forged.kg = 9_999;
    assert_eq!(pledge(&forged, &proof2), Err(Ok(Error::InvalidStatus)));
    f.client.d_release(&id);

    // C
    for no in 0..10u64 {
        let n = f.client.c_add_lot(&f.custodian, &f.verifier, &f.farmer, &f.warehouse, &sym(COMMODITY),
            &(5_000 + no), &sym(GRADE), &1_000, &10, &MOISTURE);
        assert_eq!(n as u64, no + 1);
    }
    f.client.c_pledge_lot(&f.farmer, &lender, &f.warehouse, &sym(COMMODITY), &5_003,
        &String::from_str(&env, "LOAN"), &10, &0);
}

#[test]
fn proof_check_cost() {
    let env = Env::default();
    let l = setup_lending(&env);
    let f = &l.f;
    let leaf = BytesN::from_array(&env, &[7u8; 32]);
    let mut acc = leaf.clone();
    let mut proof = Vec::new(&env);
    for i in 0..11u8 {
        let sib = BytesN::from_array(&env, &[i; 32]);
        acc = node(&env, &acc, &sib);
        proof.push_back(sib);
    }
    f.client.b_anchor(&f.custodian, &f.warehouse, &1, &acc, &2048, &true);
    let empty = Vec::new(&env);
    f.client.b_verify(&f.warehouse, &1, &leaf, &0, &empty);
    let base = env.cost_estimate().resources().instructions;
    assert!(f.client.b_verify(&f.warehouse, &1, &leaf, &0, &proof));
    let with = env.cost_estimate().resources().instructions;
    std::println!("PROOF depth 11: {} instructions over an empty proof ({} vs {})", with - base, with, base);
}
