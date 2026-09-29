use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    Address, Env, IntoVal, String,
};

use super::setup;
use crate::{Error, Status};

#[test]
fn revoke_voids_an_active_receipt() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    f.client
        .revoke(&id, &String::from_str(&env, "duplicate paper ticket"));

    let r = f.client.get_receipt(&id);
    assert_eq!(r.status, Status::Revoked);
    // Revoking records the fact, not a rewritten history: weight and lineage
    // are untouched, and the reason lives in the event.
    assert_eq!(r.weight_kg, 2_000);
    assert_eq!(r.exit_weight_kg, None);
}

#[test]
fn revoke_is_admin_only() {
    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-0001");

    let reason = String::from_str(&env, "not yours to void");
    let args = (id, reason.clone()).into_val(&env);

    // The custodian who minted it cannot void it...
    env.mock_auths(&[MockAuth {
        address: &f.custodian,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "revoke",
            args,
            sub_invokes: &[],
        },
    }]);
    assert!(f.client.try_revoke(&id, &reason).is_err());
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);

    // ...nor can a stranger...
    let stranger = Address::generate(&env);
    env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "revoke",
            args: (id, reason.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(f.client.try_revoke(&id, &reason).is_err());
    assert_eq!(f.client.get_receipt(&id).status, Status::Active);

    // ...but the admin can.
    env.mock_auths(&[MockAuth {
        address: &f.admin,
        invoke: &MockAuthInvoke {
            contract: &f.contract_id,
            fn_name: "revoke",
            args: (id, reason.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    f.client.revoke(&id, &reason);
    assert_eq!(f.client.get_receipt(&id).status, Status::Revoked);
}

#[test]
fn revoke_rejects_anything_not_active() {
    let env = Env::default();
    let f = setup(&env);
    let reason = String::from_str(&env, "too late");

    // Exited: the grain is already gone, so there is nothing to void.
    let exited = f.mint(&env, "DEP-EXITED");
    f.client.exit(&exited, &f.custodian, &2_000, &2_000);
    assert_eq!(
        f.client.try_revoke(&exited, &reason),
        Err(Ok(Error::InvalidStatus))
    );

    // Already revoked.
    let revoked = f.mint(&env, "DEP-REVOKED");
    f.client.revoke(&revoked, &reason);
    assert_eq!(
        f.client.try_revoke(&revoked, &reason),
        Err(Ok(Error::InvalidStatus))
    );

    // A retired split parent — revoke its live children instead.
    let parent = f.mint(&env, "DEP-SPLIT");
    let child = f.client.exit(&parent, &f.custodian, &500, &500);
    assert_eq!(
        f.client.try_revoke(&parent, &reason),
        Err(Ok(Error::InvalidStatus))
    );
    f.client.revoke(&(child + 1), &reason);
    assert_eq!(f.client.get_receipt(&(child + 1)).status, Status::Revoked);
}

#[test]
fn revoke_rejects_an_unknown_receipt() {
    let env = Env::default();
    let f = setup(&env);

    assert_eq!(
        f.client
            .try_revoke(&999, &String::from_str(&env, "nothing here")),
        Err(Ok(Error::ReceiptNotFound))
    );
}

#[test]
fn revoke_emits_a_revoked_event() {
    use soroban_sdk::{testutils::Events as _, Event as _};

    let env = Env::default();
    let f = setup(&env);
    let id = f.mint(&env, "DEP-EVT");
    let reason = String::from_str(&env, "fraudulent intake");

    f.client.revoke(&id, &reason);

    let expected = crate::events::Revoked {
        receipt_id: id,
        reason,
    };
    assert_eq!(
        env.events().all(),
        std::vec![expected.to_xdr(&env, &f.contract_id)]
    );
}
