"""Every outcome of verify_proof.py, against a stand-in RPC serving chosen
ledger entries. Standard library only:

    python3 -m unittest discover -s scripts -p 'test_verify_proof.py'
"""
import base64, http.server, json, os, subprocess, sys, tempfile, threading, unittest

sys.path.insert(0, os.path.dirname(__file__))
import verify_proof as vp  # noqa: E402

SCRIPT = os.path.join(os.path.dirname(__file__), "verify_proof.py")
CONTRACT = "CBW6TA5UJMXJY2VYPJV6WUVWNUP3PKZNPXFMC2CNVCWZDF3SKIT4UAIN"
ACCOUNT = "GBCF56H2FW3JOTAJFNPX6MROJSXSLNGMID3EXXAMDE6BANFOVR3XQKLR"
WH, DAY, TODAY = "TEST_WH", 20700, 20738


def leaf(i, day=DAY):
    return {"version": 1, "kind": 0, "warehouse_id": WH, "day": day, "index": i,
            "deposit_no": 100 + i, "deposit_id": f"test-{i}", "receipt_id": 0,
            "receipt_version": 0, "owner": ACCOUNT, "commodity": "MAIZE_WHITE",
            "grade": "G1", "kg": 1000 + i, "weight_source": "Scale", "bags": 10,
            "moisture_bps": 1200, "custodian": ACCOUNT, "verifier": ACCOUNT,
            "deposited_at": day * 86400, "at": day * 86400 + 60, "salt": f"{i:02x}" * 32}


def tree(leaves):  # -> root, proofs; padded with sha256(0x00) as docs/events.md says
    level = [vp.leaf_hash(l) for l in leaves]
    while len(level) & (len(level) - 1): level.append(vp.sha(b"\x00"))
    proofs = [[] for _ in leaves]
    pos = list(range(len(leaves)))
    while len(level) > 1:
        for k, p in enumerate(pos):
            proofs[k].append(level[p ^ 1]); pos[k] = p // 2
        level = [vp.sha(b"\x01", level[i], level[i + 1]) for i in range(0, len(level), 2)]
    return level[0], proofs


class Chain:
    """Ledger entries by key, served as getLedgerEntries would serve them."""

    def __init__(self):
        self.entries, self.close_time, self.fail = {}, TODAY * 86400 + 3600, None

    def put(self, key, durability, value, live_until=9_000_000):
        lk = vp.u32(6) + vp.address(CONTRACT) + key + vp.u32(durability)
        data = vp.u32(6) + vp.u32(0) + vp.address(CONTRACT) + key + vp.u32(durability) + value
        self.entries[base64.b64encode(lk).decode()] = {
            "key": base64.b64encode(lk).decode(), "xdr": base64.b64encode(data).decode(),
            "lastModifiedLedgerSeq": 5_000_000, "liveUntilLedgerSeq": live_until}

    def head(self, last_day, head, **kw):
        self.put(vp.sv("vec", [vp.sv("symbol", "Head"), vp.sv("symbol", WH)]), 1,
                 vp.sv("vec", [vp.sv("u32", last_day), vp.sv("bytes", head)]), **kw)

    def day_root(self, day, root, leaves):
        self.put(vp.sv("vec", [vp.sv("symbol", "DayRoot"), vp.sv("symbol", WH), vp.sv("u32", day)]), 0,
                 vp.sv("vec", [vp.sv("bytes", root), vp.sv("u32", leaves)]))


def serve(chain):
    class H(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            req = json.loads(self.rfile.read(int(self.headers["content-length"])))
            if chain.fail == "jsonrpc":
                body = {"jsonrpc": "2.0", "id": 1, "error": {"code": -32603, "message": "down"}}
            elif req["method"] == "getLatestLedger":
                body = {"jsonrpc": "2.0", "id": 1, "result": {"sequence": 5_100_000, "closeTime": str(chain.close_time)}}
            else:
                found = [chain.entries[k] for k in req["params"]["keys"] if k in chain.entries]
                body = {"jsonrpc": "2.0", "id": 1, "result": {"entries": found, "latestLedger": 5_100_000}}
            raw = b"<html>bad gateway</html>" if chain.fail == "html" else json.dumps(body).encode()
            self.send_response(502 if chain.fail == "http" else 200)
            self.end_headers(); self.wfile.write(raw)

        def log_message(self, *a): pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


class Outcomes(unittest.TestCase):
    def setUp(self):
        self.chain = Chain()
        self.srv = serve(self.chain)
        self.rpc = f"http://127.0.0.1:{self.srv.server_port}"
        self.leaves = [leaf(i) for i in range(5)]
        self.root, self.proofs = tree(self.leaves)
        self.prev = bytes(range(32))
        self.head = vp.chain(self.prev, DAY, 5, self.root)

    def tearDown(self):
        self.srv.shutdown(); self.srv.server_close()

    def bundle(self, i=2, **over):
        b = {"rpc": self.rpc, "contract_id": CONTRACT, "leaf": dict(self.leaves[i]),
             "proof": [p.hex() for p in self.proofs[i]], "day_leaves": 5,
             "prev_head": self.prev.hex(), "later_days": []}
        b.update(over)
        return b

    def run_script(self, b, raw=None):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            fh.write(raw if raw is not None else json.dumps(b))
        try:
            res = subprocess.run([sys.executable, SCRIPT, fh.name], capture_output=True, text=True, timeout=60)
        finally:
            os.unlink(fh.name)
        last = res.stdout.strip().splitlines()[-1]
        self.assertTrue(last.startswith("RESULT "), res.stdout + res.stderr)
        return last.split()[1], res.returncode

    def fresh(self):
        self.chain.head(DAY, self.head)
        self.chain.day_root(DAY, self.root, 5)

    def assertOutcome(self, b, code, raw=None):
        got, status = self.run_script(b, raw)
        self.assertEqual(got, code)
        self.assertEqual(status, vp.OUTCOMES[code][0])

    def test_ok(self):
        self.fresh()
        for i in range(5): self.assertOutcome(self.bundle(i), "OK")

    def test_an_archived_head_still_verifies(self):
        self.chain.head(DAY, self.head, live_until=0)
        self.chain.day_root(DAY, self.root, 5)
        self.assertOutcome(self.bundle(), "OK")

    def test_day_root_expired_verifies_through_the_head(self):
        later_root = vp.sha(b"later")
        self.chain.head(DAY + 40, vp.chain(self.head, DAY + 40, 3, later_root))
        self.assertOutcome(self.bundle(later_days=[{"day": DAY + 40, "leaves": 3, "root": later_root.hex()}]),
                           "DAY_ROOT_EXPIRED")

    def test_not_yet_anchored(self):
        self.chain.head(DAY - 1, vp.sha(b"yesterday"))
        self.assertOutcome(self.bundle(), "NOT_YET_ANCHORED")

    def test_a_day_after_the_chains_day_is_forged_not_pending(self):
        self.chain.head(DAY, self.head)
        self.chain.close_time = (DAY - 1) * 86400
        self.assertOutcome(self.bundle(), "DAY_IN_FUTURE")

    def test_no_head(self):
        self.assertOutcome(self.bundle(), "NO_HEAD")

    def test_tampered_leaf_on_a_fresh_day(self):
        self.fresh()
        b = self.bundle(); b["leaf"]["kg"] += 1
        self.assertOutcome(b, "DAY_ROOT_DIFFERS")

    def test_tampered_path(self):
        self.fresh()
        b = self.bundle(); b["proof"][0] = "00" * 32
        self.assertOutcome(b, "DAY_ROOT_DIFFERS")

    def test_leaf_count_differs(self):
        self.fresh()
        self.chain.day_root(DAY, self.root, 6)
        self.assertOutcome(self.bundle(), "DAY_ROOT_DIFFERS")

    def test_tampered_leaf_after_the_root_expired(self):
        self.chain.head(DAY, self.head)
        b = self.bundle(); b["leaf"]["grade"] = "G2"
        self.assertOutcome(b, "HEAD_MISMATCH")

    def test_unchained_root(self):  # the root is on-chain, the claimed chain to it is not
        self.fresh()
        self.assertOutcome(self.bundle(prev_head="11" * 32), "HEAD_MISMATCH")

    def test_later_days_missing(self):
        self.chain.head(DAY + 1, vp.chain(self.head, DAY + 1, 1, vp.sha(b"x")))
        self.chain.day_root(DAY, self.root, 5)
        got, _ = self.run_script(self.bundle())
        self.assertEqual(got, "HEAD_MISMATCH")

    def test_proof_does_not_fit(self):
        self.fresh()
        self.assertOutcome(self.bundle(proof=[p.hex() for p in self.proofs[2][:-1]]), "PROOF_DOES_NOT_FIT")
        b = self.bundle(); b["leaf"]["index"] = 5
        self.assertOutcome(b, "PROOF_DOES_NOT_FIT")
        self.assertOutcome(self.bundle(day_leaves=0), "PROOF_DOES_NOT_FIT")

    def test_bad_bundle(self):
        self.fresh()
        b = self.bundle(); b["leaf"]["index"] = None  # a counter-slip leaf: no position yet
        self.assertOutcome(b, "BAD_BUNDLE")
        b = self.bundle(); b["leaf"]["owner"] = ACCOUNT[:-1] + "A"
        self.assertOutcome(b, "BAD_BUNDLE")
        self.assertOutcome(None, "BAD_BUNDLE", raw="{not json")

    def test_could_not_check_is_never_a_result(self):
        self.fresh()
        for fail in ("jsonrpc", "http", "html"):
            self.chain.fail = fail
            self.assertOutcome(self.bundle(), "COULD_NOT_CHECK")
        self.chain.fail = None
        self.srv.shutdown(); self.srv.server_close()
        self.assertOutcome(self.bundle(), "COULD_NOT_CHECK")
        self.srv = serve(self.chain)  # for tearDown


if __name__ == "__main__":
    unittest.main()
