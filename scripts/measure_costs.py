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
`fl-*` identities. Every deposit id is unique per run, so it can be re-run.
"""

import argparse
import base64
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

ROLES = ["custodian", "verifier", "farmer", "farmer2", "lender", "operator", "buyer"]
STROOPS_PER_XLM = 10_000_000
DAY = 17_280  # ledgers
# Failures that mean "could not reach RPC", not "the transaction was bad".
TRANSIENT = re.compile(r"Request timeout|submission timeout|client error \(Connect\)|"
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
        for attempt in range(6):
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
                time.sleep(5 * (attempt + 1))
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
        # Reads are idempotent, so any network failure is retried.
        for attempt in range(6):
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    return json.load(r)["result"]
            except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
                log(f"{method}: {e}, retrying")
                time.sleep(5 * (attempt + 1))
        sys.exit(f"RPC {method} kept failing")

    def entry_size(self, key_b64):
        """Size in bytes of a persistent entry as rent is charged on it: the
        whole LedgerEntry XDR. Returns (size, live_until_ledger)."""
        lk = encode("LedgerKey", {"contract_data": {
            "contract": self.c, "key": decode("ScVal", key_b64),
            "durability": "persistent"}})
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

    log("registering roles (ignoring 'already registered')")
    for fn, kw in [("add_warehouse", {"warehouse_id": "WH_KANO_01"}),
                   ("add_custodian", {"warehouse_id": "WH_KANO_01", "custodian": A["custodian"]}),
                   ("add_verifier", {"verifier": A["verifier"]}),
                   ("add_recovery_operator", {"operator": A["operator"]})]:
        args = ["stellar", "contract", "invoke", "--id", net.c, "--source-account", a.admin,
                "--network", net.network, "--", fn]
        for k, v in kw.items():
            args += [f"--{k}", v]
        sh(*args, check=False)

    rows = []
    entries = {}
    deposits = {}
    n = 0

    def save():
        with open(f"deployments/{net.network}.costs.json", "w") as f:
            json.dump(rows, f, indent=2)

    def mint(farmer, kg=2000, label=None):
        nonlocal n
        n += 1
        out, h = net.invoke(ids["custodian"], "mint",
                            custodian=ids["custodian"], verifier=ids["verifier"],
                            deposit_id=f"DEP-{run}-{n}", farmer=ids[farmer],
                            warehouse_id="WH_KANO_01", commodity="MAIZE_WHITE",
                            grade="GRADE_1", weight_kg=kg, weight_source='"Scale"',
                            bag_count=20, moisture_pct=1250)
        if label:
            rows.append(costs(net, label, h))
        deposits[int(out)] = f"DEP-{run}-{n}"
        return int(out)

    def receipt_key(rid):
        return scval_key("Receipt", {"u64": str(rid)})

    def deposit_key(rid):
        return scval_key("DepositIndex", {"string": deposits[rid]})

    def size(label, key):
        b, live_until = net.entry_size(key)
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
    log("mint (new farmer)")
    mint("fresh", label="mint — farmer's first receipt (creates owner index)")
    log("mint (existing farmer)")
    r1 = mint("farmer", label="mint — farmer's later receipt")
    size("Receipt — Active, freshly minted", receipt_key(r1))
    size("DepositIndex", deposit_key(r1))
    r2 = mint("farmer")
    r3 = mint("farmer")

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
    *batch, spare1, spare2 = [mint("farmer2", kg=1000) for _ in range(a.batch + 2)]
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
    extend(f"native extend — {a.batch} receipts + deposit guards, to 180-day ceiling",
           [k for r in batch for k in (receipt_key(r), deposit_key(r))], 180 * DAY - 1)

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
