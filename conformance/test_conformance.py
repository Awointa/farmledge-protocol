"""Self-tests of the conformance checks: each invariant has a test that
models a backend without its enforcement, and asserts that exactly the
expected invariants fail. A check that stopped catching its flaw fails
here. Run: python3 -m unittest conformance/test_conformance.py
"""
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import example  # noqa: E402
import farmledge_conformance as fc  # noqa: E402


def failing(flaw=None, **kw):
    data = example.build(flaw, **kw)
    return {inv for inv, problems in fc.check_all(data, data["as_of"]).items() if problems}


class Conformance(unittest.TestCase):
    def test_a_conforming_backend_passes_every_invariant(self):
        self.assertEqual(failing(), set())

    def test_a_conforming_backend_beside_an_older_contract_passes(self):
        self.assertEqual(failing(older_contract=True), set())

    def test_i1_without_the_freeze_a_stale_mint_is_caught(self):
        # Caught twice over: the dispatch inside the freeze window (I1), and
        # the anchored balance no longer matching the receipt (I6).
        self.assertEqual(failing("I1"), {"I1", "I6"})

    def test_i2_a_mint_before_the_preclaim_gate_is_caught(self):
        self.assertEqual(failing("I2"), {"I2"})

    def test_i2_a_live_lot_on_the_older_contract_left_unclaimed_is_caught(self):
        self.assertEqual(failing("I2-unlisted"), {"I2"})

    def test_i2_a_preclaimed_lot_minted_anyway_is_caught(self):
        self.assertEqual(failing("I2-old-minted"), {"I2"})

    def test_i3_a_gap_in_deposit_numbers_is_caught(self):
        self.assertEqual(failing("I3"), {"I3"})

    def test_i4_incomplete_books_are_caught(self):
        self.assertEqual(failing("I4"), {"I4"})

    def test_i4_a_reused_salt_is_caught(self):
        self.assertEqual(failing("I4-salt"), {"I4"})

    def test_i5_a_skipped_day_is_caught(self):
        self.assertEqual(failing("I5"), {"I5"})

    def test_i5_a_missed_anchor_is_caught(self):
        self.assertEqual(failing("I5-missed"), {"I5"})

    def test_i6_a_minted_lot_dispatched_off_chain_is_caught(self):
        self.assertEqual(failing("I6"), {"I6"})

    def test_i7_an_undelivered_proof_is_caught(self):
        self.assertEqual(failing("I7"), {"I7"})

    def test_i7_a_late_proof_is_caught(self):
        self.assertEqual(failing("I7-late"), {"I7"})

    def test_i8_an_indexer_gap_is_caught(self):
        self.assertEqual(failing("I8"), {"I8"})

    def test_i9_a_keepalive_naming_an_exited_receipt_is_caught(self):
        self.assertEqual(failing("I9"), {"I9"})

    def test_every_flaw_is_tested(self):
        named = {m.split("_")[1] for m in dir(self) if m.startswith("test_i")}
        self.assertEqual({f.split("-")[0].lower() for f in example.FLAWS}, named)

    def test_the_cli_exits_nonzero_on_any_failure(self):
        path = os.path.join(HERE, "example.export.json")
        cli = [sys.executable, os.path.join(HERE, "farmledge_conformance.py")]
        self.assertEqual(subprocess.run(cli + [path], capture_output=True).returncode, 0)
        bad = example.build("I9")
        tmp = os.path.join(HERE, ".bad.json")
        try:
            with open(tmp, "w") as fh:
                json.dump(bad, fh)
            self.assertEqual(subprocess.run(cli + [tmp], capture_output=True).returncode, 1)
        finally:
            os.unlink(tmp)

    def test_the_committed_example_is_current(self):
        with open(os.path.join(HERE, "example.export.json")) as fh:
            self.assertEqual(json.load(fh), example.build())


TESTNET_EXPORT = os.path.join(HERE, "testnet.export.json")


@unittest.skipUnless(os.environ.get("FARMLEDGE_TESTNET") and os.path.exists(TESTNET_EXPORT),
                     "set FARMLEDGE_TESTNET=1 after running testnet_example.py")
class AgainstTestnet(unittest.TestCase):
    """The --rpc checks against the deployed contract: the real export
    passes, and exports tampered so that they still pass offline are caught
    only by the chain."""

    def setUp(self):
        with open(TESTNET_EXPORT) as fh:
            self.data = json.load(fh)
        with open(os.path.join(HERE, "..", "deployments", "testnet.json")) as fh:
            self.rpc = json.load(fh)["rpc_url"]

    def failing(self, data, rpc=True):
        res = fc.check_all(data, data["as_of"], self.rpc if rpc else None)
        return {inv for inv, problems in res.items() if problems}

    def test_the_real_export_passes_against_the_chain(self):
        self.assertEqual(self.failing(self.data), set())

    def test_i8_an_event_missing_from_the_archive_is_caught_only_on_chain(self):
        events = self.data["indexer"]["events"]
        gone = next(i for i, e in enumerate(events) if e["type"] == "ttl_extended"
                    and e["fields"]["warehouse_id"] != self.data["warehouses"][0]["warehouse_id"])
        del events[gone]
        self.assertEqual(self.failing(self.data, rpc=False), set())
        self.assertEqual(self.failing(self.data), {"I8"})

    def test_i4_i5_i7_consistently_forged_books_are_caught_only_on_chain(self):
        wh = self.data["warehouses"][0]["warehouse_id"]
        day = self.data["days"][-1]
        day["leaves"][0]["salt"] = "ab" * 32  # rewrite history, then cover the tracks
        root, proof = fc.merkle([fc.vp.leaf_hash(l) for l in day["leaves"]])
        mine = [e["fields"] for e in self.data["indexer"]["events"]
                if e["type"] == "anchored" and e["fields"]["warehouse_id"] == wh]
        a = next(f for f in mine if f["day"] == day["day"])
        a["root"] = root.hex()
        a["head"] = fc.vp.chain(bytes.fromhex(a["prev_head"]), a["day"], a["leaves"], root).hex()
        for dl in self.data["deliveries"]:
            if dl["day"] == day["day"]:
                dl["bundle"]["leaf"] = day["leaves"][dl["index"]]
                dl["bundle"]["proof"] = [p.hex() for p in proof(dl["index"])]
        self.assertEqual(self.failing(self.data, rpc=False), set())
        self.assertEqual(self.failing(self.data), {"I4", "I5", "I7"})

    def test_i2_a_preclaim_not_on_chain_is_caught_only_on_chain(self):
        wh = self.data["warehouses"][0]["warehouse_id"]
        self.data["preclaim"]["old_contract"] = {
            "contract_id": example.OLD_CONTRACT,
            "live_lots": [{"warehouse_id": wh, "deposit_no": 999, "receipt_id": 7}]}
        last = self.data["indexer"]["events"][0]
        self.data["indexer"]["events"].append(dict(last, id=last["id"] + "x", type="preclaimed",
            fields={"warehouse_id": wh, "deposit_no": 999, "claimed_at": 0}))
        self.assertEqual(self.failing(self.data, rpc=False), set())
        self.assertEqual(self.failing(self.data), {"I2"})


if __name__ == "__main__":
    unittest.main()
