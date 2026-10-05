#!/usr/bin/env python3
"""Measure what each contract operation actually costs on a live network.

    scripts/measure_costs.py                 # uses deployments/testnet.json
    scripts/measure_costs.py --batch 25      # size of the extend_ttl_batch run

Runs one realistic scenario against the deployed contract, submitting real
transactions, then reads each transaction back from RPC and reports what the
network charged: total fee, its resource-fee split, rent, and the resources
the transaction declared. Prints a Markdown table to stdout; progress goes to
stderr.

Needs stellar-cli and the admin identity named in the deployment record (it
must be the contract admin, as on Testnet). Creates and funds its own
`fl-*` identities. Each run registers its own warehouse, so its anchors
start a fresh chain, and it can be re-run.

Minting is on demand (`data_version` 3): each run anchors two days of a
warehouse's books as 2,048-leaf trees (a warehouse with about 2,000 active
lots, proof depth 11), then mints from the latest. Leaf hashes and proofs
are computed with `scripts/verify_proof.py`, the standalone verifier
published in `docs/events.md`, which the run then uses to check two proof
bundles against the chain.
"""

import argparse
import base64
import json
import os
import re
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import verify_proof as vp  # noqa: E402

ROLES = ["custodian", "verifier", "farmer", "farmer2", "lender", "operator", "buyer"]
STROOPS_PER_XLM = 10_000_000
DAY = 17_280  # ledgers
# Failures that mean "could not reach RPC", not "the transaction was bad".
TRANSIENT = re.compile(r"Request timeout|submission timeout|client error \(Connect\)|dns error|"
                       r"error sending request|connection (reset|closed|refused)|"
                       r"timed out|\b50[234]\b", re.I)


def log(msg):
    print(f"==> {msg}", file=sys.stderr, flush=True)


def sh(*args, check=True):
    res = subprocess.run(args, capture_output=True, text=True)
    if check and res.returncode != 0:
        sys.exit(f"command failed: {' '.join(args)}\n{res.stderr}")
    return res


class Net:
    def __init__(self, record):
        self.c = record["contract_id"]
        self.network = record["network"]
        self.rpc = record["rpc_url"]

    def addr(self, ident):
        return sh("stellar", "keys", "address", ident).stdout.strip()

    def invoke(self, source, fn, **kwargs):
        """Submits one invocation. Returns (return value, tx hash)."""
        args = ["stellar", "contract", "invoke", "--id", self.c,
                "--source-account", source, "--network", self.network,
                "--send", "yes", "--", fn]
        for k, v in kwargs.items():
            args += [f"--{k}", str(v)]
        return self.submit(fn, args)

    def extend(self, source, keys, ledgers):
        """Extends contract-data entries with a native ExtendFootprintTTL
        operation, no contract call. Returns the tx hash."""
        args = ["stellar", "contract", "extend", "--id", self.c,
                "--source-account", source, "--network", self.network,
                "--ledgers-to-extend", str(ledgers)]
        for k in keys:
            args += ["--key-xdr", k]
        return self.submit("extend", args)[1]

    def submit(self, label, args):
        """Runs a submitting CLI command, retrying transient RPC failures
        without ever applying a transaction twice. Returns (stdout, hash)."""
        for attempt in range(10):
            res = sh(*args, check=False)
            hashes = re.findall(r"Signing transaction: ([0-9a-f]{64})", res.stderr)
            if res.returncode == 0 and hashes:
                return res.stdout.strip(), hashes[-1]
            if not TRANSIENT.search(res.stderr):
                break
            if not hashes:
                # Failed before signing (simulation or sequence fetch):
                # nothing was submitted, so retrying is safe.
                log(f"{label}: RPC unreachable before signing, retrying")
                time.sleep(min(5 * (attempt + 1), 30))
                continue
            # Signed, then lost the connection: the transaction may or may
            # not have landed. Ask RPC; retry only once it is known absent.
            # A resubmission reuses the same sequence number unless the
            # first one applied, so the two can never both apply.
            log(f"{label}: lost contact after signing, polling RPC for {hashes[-1][:12]}")
            t = self.tx(hashes[-1], wait=60, missing_ok=True)
            if t is not None:
                meta = decode("TransactionMeta", t["resultMetaXdr"])
                ret = find(meta, "return_value") or {}
                out = str(next(iter(ret.values()), "")) if isinstance(ret, dict) else ""
                return out, hashes[-1]
            log(f"{label}: {hashes[-1][:12]} never landed, resubmitting")
        sys.exit(f"command failed: {' '.join(args)}\n{res.stderr}")

    def rpc_call(self, method, params):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method,
                           "params": params}).encode()
        # The public Testnet RPC rejects urllib's default User-Agent.
        req = urllib.request.Request(self.rpc, body,
                                     {"content-type": "application/json",
                                      "user-agent": "farmledge-measure-costs/1"})
        # Reads are idempotent, so any network failure is retried, for about
        # five minutes in all: long enough to ride out a DNS outage.
        for attempt in range(12):
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    return json.load(r)["result"]
            except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
                log(f"{method}: {e}, retrying")
                time.sleep(min(5 * (attempt + 1), 30))
        sys.exit(f"RPC {method} kept failing")

    def entry_size(self, key_b64, durability="persistent"):
        """Size in bytes of an entry as rent is charged on it: the whole
        LedgerEntry XDR. Returns (size, live_until_ledger)."""
        lk = encode("LedgerKey", {"contract_data": {
            "contract": self.c, "key": decode("ScVal", key_b64),
            "durability": durability}})
        e = self.rpc_call("getLedgerEntries", {"keys": [lk]})["entries"][0]
        # LedgerEntry = data + lastModifiedLedgerSeq (4) + ext discriminant (4).
        return len(base64.b64decode(e["xdr"])) + 8, e["liveUntilLedgerSeq"]

    def tx(self, tx_hash, wait=120, missing_ok=False):
        for _ in range(wait // 2):
            t = self.rpc_call("getTransaction", {"hash": tx_hash})
            if t["status"] == "SUCCESS":
                return t
            if t["status"] == "FAILED":
                sys.exit(f"transaction {tx_hash} failed")
            time.sleep(2)
        if missing_ok:
            return None
        sys.exit(f"transaction {tx_hash} not found")


def decode(xdr_type, b64):
    res = subprocess.run(
        ["stellar", "xdr", "decode", "--type", xdr_type, "--output", "json"],
        input=b64, capture_output=True, text=True, check=True)
    return json.loads(res.stdout)


def encode(xdr_type, obj):
    res = subprocess.run(
        ["stellar", "xdr", "encode", "--type", xdr_type],
        input=json.dumps(obj), capture_output=True, text=True, check=True)
    return res.stdout.strip()


def scval_key(*parts):
    """Base64 ScVal of a `DataKey` variant, e.g. scval_key("Receipt", u64=7)."""
    name, *vals = parts
    return encode("ScVal", {"vec": [{"symbol": name}] + list(vals)})


def find(obj, key):
    """First value stored under `key` anywhere in a decoded XDR tree."""
    if isinstance(obj, dict):
        if key in obj:
            return obj[key]
        for v in obj.values():
            hit = find(v, key)
            if hit is not None:
                return hit
    elif isinstance(obj, list):
        for v in obj:
            hit = find(v, key)
            if hit is not None:
                return hit
    return None


def costs(net, label, tx_hash):
    t = net.tx(tx_hash)
    env = decode("TransactionEnvelope", t["envelopeXdr"])
    meta = decode("TransactionMeta", t["resultMetaXdr"])
    result = decode("TransactionResult", t["resultXdr"])
    resources = find(env, "resources")
    fp = resources["footprint"]
    # Contract events count against the 16KB per-transaction event limit,
    # which is what bounds MAX_TTL_BATCH.
    groups = (t.get("events") or {}).get("contractEventsXdr") or []
    event_bytes = sum(len(base64.b64decode(e)) for g in groups for e in g)
    return {
        "op": label,
        "fee": int(result["fee_charged"]),
        "non_refundable": int(find(meta, "total_non_refundable_resource_fee_charged") or 0),
        "refundable": int(find(meta, "total_refundable_resource_fee_charged") or 0),
        "rent": int(find(meta, "rent_fee_charged") or 0),
        "instructions": resources["instructions"],
        "read_bytes": resources["disk_read_bytes"],
        "write_bytes": resources["write_bytes"],
        "ro": len(fp["read_only"]),
        "rw": len(fp["read_write"]),
        "event_bytes": event_bytes,
        "ledger": t["ledger"],
        "tx": tx_hash,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--deployment", default="deployments/testnet.json")
    ap.add_argument("--admin", default="farmledge-admin")
    ap.add_argument("--batch", type=int, default=25)
    a = ap.parse_args()

    net = Net(json.load(open(a.deployment)))
    run = str(int(time.time()))
    ids = {r: f"fl-{r}" for r in ROLES}
    for ident in ids.values():
        if sh("stellar", "keys", "address", ident, check=False).returncode != 0:
            sh("stellar", "keys", "generate", ident, "--network", net.network)
        sh("stellar", "keys", "fund", ident, "--network", net.network, check=False)
    A = {r: net.addr(i) for r, i in ids.items()}

    wh = f"WH_M{run}"
    log(f"registering roles for {wh} (ignoring 'already registered')")
    for fn, kw in [("add_warehouse", {"warehouse_id": wh}),
                   ("add_custodian", {"warehouse_id": wh, "custodian": A["custodian"]}),
                   ("add_verifier", {"verifier": A["verifier"]}),
                   ("add_recovery_operator", {"operator": A["operator"]})]:
        args = ["stellar", "contract", "invoke", "--id", net.c, "--source-account", a.admin,
                "--network", net.network, "--", fn]
        for k, v in kw.items():
            args += [f"--{k}", v]
        sh(*args, check=False)

    rows = []
    entries = {}

    def save():
        with open(f"deployments/{net.network}.costs.json", "w") as f:
            json.dump(rows, f, indent=2)

    def receipt_key(rid):
        return scval_key("Receipt", {"u64": str(rid)})

    def size(label, key, durability="persistent"):
        b, live_until = net.entry_size(key, durability)
        latest = net.rpc_call("getLatestLedger", {})["sequence"]
        entries[label] = {"bytes": b, "live_until": live_until, "at_ledger": latest}
        with open(f"deployments/{net.network}.entries.json", "w") as f:
            json.dump(entries, f, indent=2)

    def extend(label, keys, ledgers):
        log(label)
        before = [net.entry_size(k) for k in keys]
        h = net.extend(ids["buyer"], keys, ledgers)
        row = costs(net, label, h)
        # What the rent formula needs: each entry's size and how many
        # ledgers of TTL the operation actually bought for it.
        row["extended"] = [{"bytes": b, "ledgers": row["ledger"] + ledgers - lu}
                           for b, lu in before if row["ledger"] + ledgers > lu]
        rows.append(row)
        save()

    def op(label, source, fn, **kw):
        log(label)
        out, h = net.invoke(source, fn, **kw)
        rows.append(costs(net, label, h))
        save()
        return out

    # A farmer nobody has minted for yet: the mint also creates their index.
    fresh = f"fl-farmer-{run}"
    sh("stellar", "keys", "generate", fresh, "--network", net.network)
    sh("stellar", "keys", "fund", fresh, "--network", net.network)
    ids["fresh"] = fresh
    A["fresh"] = net.addr(fresh)

    # --- the books: every lot this run mints, anchored on two days -------
    owners = ["fresh", "farmer", "farmer", "farmer"] + ["farmer2"] * (a.batch + 2)
    depth = 11
    day_a = int(time.time()) // 86_400 - 2

    def books(day):
        leaves = [{"version": 1, "kind": 2, "warehouse_id": wh, "day": day, "index": i,
                   "deposit_no": i + 1, "deposit_id": f"DEP-{run}-{i + 1}", "receipt_id": 0,
                   "receipt_version": 0, "owner": A[o], "commodity": "MAIZE_WHITE",
                   "grade": "GRADE_1", "kg": 1000 if o == "farmer2" else 2000,
                   "weight_source": "Scale", "bags": 20, "moisture_bps": 1250,
                   "custodian": A["custodian"], "verifier": A["verifier"],
                   "deposited_at": day * 86_400 - 3_600, "at": (day + 1) * 86_400,
                   "salt": secrets.token_hex(32)} for i, o in enumerate(owners)]
        # The rest of the warehouse: only the real leaves' paths matter.
        hashes = [vp.leaf_hash(l) for l in leaves]
        hashes += [secrets.token_bytes(32) for _ in range(2 ** depth - len(hashes))]
        levels = [hashes]
        while len(levels[-1]) > 1:
            cur = levels[-1]
            levels.append([vp.sha(b"\x01", cur[i], cur[i + 1]) for i in range(0, len(cur), 2)])
        proofs = []
        for i in range(len(leaves)):
            proof, j = [], i
            for level in levels[:-1]:
                proof.append(level[j ^ 1])
                j //= 2
            proofs.append(proof)
        return leaves, proofs, levels[-1][0]

    # The verifier's leaf hash must be the contract's own.
    leaves_a, proofs_a, root_a = books(day_a)
    sim = sh("stellar", "contract", "invoke", "--id", net.c, "--source-account", ids["buyer"],
             "--network", net.network, "--send=no", "--", "leaf_hash",
             "--leaf", json.dumps(leaves_a[0])).stdout.strip().strip('"')
    if sim != vp.leaf_hash(leaves_a[0]).hex():
        sys.exit(f"leaf_hash mismatch: contract {sim}, verifier {vp.leaf_hash(leaves_a[0]).hex()}")

    head_a = op("anchor — first day (creates the head), 2,048 leaves", ids["custodian"],
                "anchor", custodian=ids["custodian"], warehouse_id=wh, day=day_a,
                root=root_a.hex(), leaves=2 ** depth).strip('"')
    day_b = day_a + 1
    leaves_b, proofs_b, root_b = books(day_b)
    op("anchor — next day, 2,048 leaves", ids["custodian"], "anchor",
       custodian=ids["custodian"], warehouse_id=wh, day=day_b, root=root_b.hex(),
       leaves=2 ** depth)
    size("Head", scval_key("Head", {"symbol": wh}))
    size("DayRoot (temporary)", scval_key("DayRoot", {"symbol": wh}, {"u32": day_b}),
         "temporary")

    # Two proof bundles, checked with the standalone verifier against the
    # chain: today's (day root on-chain) and yesterday's through the head.
    for label, leaf, proof, prev, later in [
            ("latest day", leaves_b[1], proofs_b[1], head_a, []),
            ("earlier day, through the head chain", leaves_a[1], proofs_a[1], "00" * 32,
             [{"day": day_b, "leaves": 2 ** depth, "root": root_b.hex()}])]:
        bundle = {"rpc": net.rpc, "contract_id": net.c, "leaf": leaf,
                  "proof": [p.hex() for p in proof], "day_leaves": 2 ** depth,
                  "prev_head": prev, "later_days": later}
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            json.dump(bundle, fh)
        res = sh(sys.executable, os.path.join(os.path.dirname(__file__), "verify_proof.py"),
                 fh.name, check=False)
        os.unlink(fh.name)
        log(f"verify_proof.py, {label}:\n{res.stdout.strip()}")
        if res.returncode != 0:
            sys.exit(f"verify_proof.py rejected a genuine bundle:\n{res.stdout}{res.stderr}")
    if os.path.exists("docs/events.md"):
        doc = open("docs/events.md").read()
        if open(os.path.join(os.path.dirname(__file__), "verify_proof.py")).read() not in doc:
            log("WARNING: docs/events.md does not embed the current verify_proof.py")

    def mint(i, label=None):
        out, h = net.invoke(ids["custodian"], "mint_from_anchor",
                            custodian=ids["custodian"], verifier=ids["verifier"],
                            leaf=json.dumps(leaves_b[i]),
                            proof=json.dumps([p.hex() for p in proofs_b[i]]))
        if label:
            rows.append(costs(net, label, h))
            save()
        return int(out)

    log("mint_from_anchor (new farmer, new claim chunk)")
    mint(0, "mint_from_anchor — first in its claim chunk (creates chunk, owner index), depth 11")
    log("mint_from_anchor (existing farmer)")
    r1 = mint(1, "mint_from_anchor — later lot, depth 11")
    size("Receipt — Active, freshly minted", receipt_key(r1))
    size("Claimed chunk (1,024 deposit numbers)", scval_key("Claimed", {"symbol": wh}, {"u64": "0"}))
    r2 = mint(2)
    r3 = mint(3)

    # Pre-claims (cutover, audit finding A19): a full batch into a new
    # chunk, then into the same chunk.
    op("preclaim — 25 numbers, creates their chunk", a.admin, "preclaim",
       warehouse_id=wh, deposit_nos=json.dumps(list(range(1100, 1125))))
    op("preclaim — 25 numbers, chunk exists", a.admin, "preclaim",
       warehouse_id=wh, deposit_nos=json.dumps(list(range(1125, 1150))))

    due = int(time.time()) + 90
    locked = int(op("lock — full", ids["farmer"], "lock", receipt_id=r1,
                    lender=ids["lender"], kg=2000, loan_ref="LOAN-M1",
                    due_date=due, grace_period=0))
    part = int(op("lock — partial (splits)", ids["farmer"], "lock", receipt_id=r2,
                  lender=ids["lender"], kg=1500, loan_ref="LOAN-M2",
                  due_date=due, grace_period=0))
    size("Receipt — Locked", receipt_key(locked))
    # A split happens in place: r2 keeps the unpledged 500kg under its own
    # id, and `part` is the new, pledged receipt.
    size("Receipt — Active, reduced in place by a split", receipt_key(r2))
    op("exit — full", ids["farmer"], "exit", receipt_id=r2,
       custodian=ids["custodian"], kg=500, exit_weight_kg=495)
    gone = int(op("exit — partial (splits)", ids["farmer"], "exit", receipt_id=r3,
                  custodian=ids["custodian"], kg=600, exit_weight_kg=598))
    op("extend_ttl_batch — 2 Exited receipts (skipped)", ids["buyer"],
       "extend_ttl_batch", receipt_ids=json.dumps([r2, gone]))
    op("extend_due", ids["lender"], "extend_due", receipt_id=part, new_due_date=due + 1)
    op("unlock — partial (splits)", ids["lender"], "unlock", receipt_id=part, kg=500)

    op("extend_ttl — 1 receipt, TTL above threshold (no-op bump)", ids["buyer"],
       "extend_ttl", receipt_id=r3)

    log(f"minting {a.batch} receipts for the batch run")
    # Two spare receipts for the single-entry extends below.
    *batch, spare1, spare2 = [mint(4 + i) for i in range(a.batch + 2)]
    op(f"extend_ttl_batch — {a.batch} receipts, TTL above threshold", ids["buyer"],
       "extend_ttl_batch", receipt_ids=json.dumps(batch))
    op("extend_index_ttl — 2 pages", ids["buyer"], "extend_index_ttl",
       index=json.dumps({"Owner": A["farmer2"]}), first_page=0, page_count=2)
    size("OwnerIndex page 0",
         scval_key("OwnerIndex", {"address": A["farmer2"]}, {"u32": 0}))

    # Rent is linear in entry size x ledgers bought. Every entry this
    # contract writes starts at 175 days, so a native extend to 176 and to
    # 180 days (the network ceiling) buys about 1 and 5 days: two points
    # that pin the formula down. 30/90-day bumps are then computed from it.
    # A bump can reach at most max_entry_ttl - 1 ledgers; one more is
    # rejected as malformed.
    extend("native extend — 1 receipt, +1 day (to 176 days)",
           [receipt_key(spare1)], 176 * DAY)
    extend("native extend — 1 receipt, +5 days (to 180-day ceiling)",
           [receipt_key(spare2)], 180 * DAY - 1)
    extend(f"native extend — {a.batch} receipts, to 180-day ceiling",
           [receipt_key(r) for r in batch], 180 * DAY - 1)

    log("waiting for the loan to fall due")
    while int(time.time()) <= due + 1 + 10:
        time.sleep(5)
    op("start_recovery", ids["lender"], "start_recovery", receipt_id=locked)
    # The lender's signature is checked against the lender recorded in the
    # lock, which the CLI cannot see from the arguments: make the lender the
    # transaction source so source-account auth covers it.
    sale = int(op("complete_recovery", ids["lender"], "complete_recovery",
                  receipt_id=locked, operator=ids["operator"], buyer=ids["buyer"],
                  sale_ref="SALE-M1"))
    size("Receipt — after a completed recovery", receipt_key(locked))
    op("record_settlement", ids["lender"], "record_settlement", sale_id=sale,
       operator=ids["operator"], settlement_ref="PAY-M1")
    size("Sale — settled", scval_key("Sale", {"u64": str(sale)}))
    # The partial unlock left 1,000kg still pledged under `part` itself.
    op("unlock — full", ids["lender"], "unlock", receipt_id=part, kg=1000)

    print(f"Network: {net.network}, contract `{net.c}`, run {run}.\n")
    print("| operation | fee charged (stroops) | XLM | of which rent | other resource fee "
          "| CPU instructions | disk read B | write B | footprint RO / RW | events B | tx |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|")
    for r in rows:
        other = r["non_refundable"] + r["refundable"] - r["rent"]
        print(f"| {r['op']} | {r['fee']:,} | {r['fee'] / STROOPS_PER_XLM:.4f} | "
              f"{r['rent']:,} | {other:,} | {r['instructions']:,} | {r['read_bytes']:,} | "
              f"{r['write_bytes']:,} | {r['ro']} / {r['rw']} | {r['event_bytes']:,} | "
              f"`{r['tx'][:12]}` |")
    save()
    log(f"raw numbers in deployments/{net.network}.costs.json")


if __name__ == "__main__":
    main()
