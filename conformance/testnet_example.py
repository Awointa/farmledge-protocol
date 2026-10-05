#!/usr/bin/env python3
"""Plays a minimal, conforming backend against the deployed v3 contract on
Testnet, exports its records, and checks them with --rpc. SYNTHETIC TEST
DATA on Testnet: a fresh warehouse per run, Testnet XLM only.

    python3 conformance/testnet_example.py   # writes conformance/testnet.export.json

Needs stellar-cli, the `farmledge-admin` identity (the contract admin on
Testnet) and the `fl-custodian`, `fl-verifier`, `fl-farmer`, `fl-buyer`
identities scripts/measure_costs.py creates.

Two closed days are anchored (day D: two lots deposited; day D+1: lot 1
part-dispatched off-chain). Lot 2 is then frozen and minted from day D+1's
balance, and 500 kg of it exits on-chain. The event archive is every event
the contract has emitted, read back from RPC, as an indexer would.
"""
import json
import os
import secrets
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import farmledge_conformance as fc  # noqa: E402
import measure_costs as m  # noqa: E402

DAY = fc.DAY


def main():
    record = json.load(open(os.path.join(ROOT, "deployments", "testnet.json")))
    net = m.Net(record)
    rpc = fc.Rpc(record["rpc_url"])
    ids = {r: f"fl-{r}" for r in ("custodian", "verifier", "farmer", "buyer")}
    A = {r: net.addr(i) for r, i in ids.items()}
    run = int(time.time())
    wh = f"WH_CF{run % 10**8}"
    D = run // DAY - 2  # days D and D+1 are closed

    def admin(fn, **kw):
        args = ["stellar", "contract", "invoke", "--id", net.c, "--source-account",
                "farmledge-admin", "--network", "testnet", "--", fn]
        for k, v in kw.items():
            args += [f"--{k}", str(v)]
        m.sh(*args)

    m.log(f"registering {wh}")
    admin("add_warehouse", warehouse_id=wh)
    admin("add_custodian", warehouse_id=wh, custodian=A["custodian"])

    lots = [{"warehouse_id": wh, "deposit_no": no, "deposit_id": f"DEP-CF-{run}-{no}",
             "owner": A["farmer"], "commodity": "MAIZE_WHITE", "grade": "GRADE_1",
             "weight_source": "Scale", "bags": kg // 100, "moisture_bps": 1250,
             "custodian": A["custodian"], "verifier": A["verifier"], "kg": kg,
             "intake_at": D * DAY + (7 + no) * 3_600} for no, kg in ((1, 2_000), (2, 1_500))]
    dispatches = [{"id": "DSP-1", "warehouse_id": wh, "deposit_no": 1, "kg": 300,
                   "recorded_at": (D + 1) * DAY + 14 * 3_600, "via": "off_chain"}]
    data = {"format": 1, "contract_id": net.c, "network": "testnet",
            "warehouses": [{"warehouse_id": wh}], "lots": lots, "dispatches": dispatches,
            "mint_requests": [], "preclaim": {"old_contract": None, "verified_at": {wh: D * DAY}},
            "days": [], "deliveries": [], "keepalive_submissions": [], "receipts": [],
            "indexer": {"start_ledger": record["start_ledger"], "events": []}}
    x = fc.Export(data)

    anchors = []
    for d in (D, D + 1):
        leaves = []
        for kind, no, kg in fc.expected_books(x, wh, d):
            lot = x.lots[(wh, no)]
            when = {0: lot["intake_at"], 2: fc.cutoff(d)}.get(kind) or next(
                z["recorded_at"] for z in dispatches if z["deposit_no"] == no)
            leaves.append({"version": 1, "kind": kind, "warehouse_id": wh, "day": d, "index": 0,
                           "deposit_no": no, "deposit_id": lot["deposit_id"], "receipt_id": 0,
                           "receipt_version": 0, "owner": lot["owner"], "commodity": lot["commodity"],
                           "grade": lot["grade"], "kg": kg, "weight_source": lot["weight_source"],
                           "bags": max(kg // 100, 1), "moisture_bps": lot["moisture_bps"],
                           "custodian": lot["custodian"], "verifier": lot["verifier"],
                           "deposited_at": lot["intake_at"], "at": when,
                           "salt": secrets.token_hex(32)})
        leaves.sort(key=fc.leaf_order)
        for i, leaf in enumerate(leaves):
            leaf["index"] = i
        root, proof = fc.merkle([fc.vp.leaf_hash(l) for l in leaves])
        m.log(f"anchor day {d}, {len(leaves)} leaves")
        net.invoke(ids["custodian"], "anchor", custodian=ids["custodian"], warehouse_id=wh,
                   day=d, root=root.hex(), leaves=len(leaves))
        data["days"].append({"warehouse_id": wh, "day": d, "leaves": leaves})
        anchors.append((d, leaves, root, proof))

    # The freeze is written before the mint is built (backend §5).
    d1, leaves, root, proof = anchors[1]
    i = next(n for n, l in enumerate(leaves) if l["deposit_no"] == 2 and l["kind"] == 2)
    frozen_at = int(time.time())
    m.log("mint_from_anchor, lot 2")
    out, _ = net.invoke(ids["custodian"], "mint_from_anchor", custodian=ids["custodian"],
                        verifier=ids["verifier"], leaf=json.dumps(leaves[i]),
                        proof=json.dumps([p.hex() for p in proof(i)]))
    rid = int(out)
    resolved = int(time.time())
    data["mint_requests"].append({"id": "MINT-1", "warehouse_id": wh, "deposit_no": 2,
                                  "anchor_day": d1, "frozen_at": frozen_at,
                                  "submitted_at": frozen_at + 1, "max_time": frozen_at + 300,
                                  "resolved_at": resolved, "outcome": "minted", "receipt_id": rid})
    m.log("exit 500 kg on-chain")
    out, _ = net.invoke(ids["farmer"], "exit", receipt_id=rid, custodian=ids["custodian"],
                        kg=500, exit_weight_kg=497)
    taken = int(out)
    dispatches.append({"id": "DSP-2", "warehouse_id": wh, "deposit_no": 2, "kg": 500,
                       "recorded_at": int(time.time()), "via": "exit", "receipt_id": taken})
    m.log("keep-alive")
    net.invoke(ids["buyer"], "extend_ttl_batch", receipt_ids=json.dumps([rid]))
    data["keepalive_submissions"].append({"submitted_at": int(time.time()), "receipt_ids": [rid]})
    data["receipts"] = [
        {"receipt_id": rid, "status": "Active", "weight_kg": 1_000, "deposit_no": 2, "warehouse_id": wh},
        {"receipt_id": taken, "status": "Exited", "weight_kg": 500, "deposit_no": 2, "warehouse_id": wh},
    ]

    m.log("archiving every contract event from RPC")
    time.sleep(6)
    cursor = rpc.latest_ledger()
    names = fc.topic_names()
    events = [fc.archive_event(e, names) for e in rpc.events(net.c, record["start_ledger"], cursor)]
    data["indexer"].update(events=events, cursor=cursor,
                           ledger_ranges=[[record["start_ledger"], cursor]])

    mine = sorted((e["fields"] for e in events
                   if e["type"] == "anchored" and e["fields"]["warehouse_id"] == wh),
                  key=lambda a: a["day"])
    for n, (d, leaves, root, proof) in enumerate(anchors):
        a = mine[n]
        later = [{"day": b["day"], "leaves": b["leaves"], "root": b["root"]} for b in mine[n + 1:]]
        for i, leaf in enumerate(leaves):
            data["deliveries"].append({
                "warehouse_id": wh, "day": d, "index": i, "channel": "sms",
                "delivered_at": a["anchored_at"] + 60,
                "bundle": {"rpc": record["rpc_url"], "contract_id": net.c, "leaf": leaf,
                           "proof": [p.hex() for p in proof(i)], "day_leaves": len(leaves),
                           "prev_head": a["prev_head"], "later_days": later}})
    data["as_of"] = int(time.time())
    path = os.path.join(HERE, "testnet.export.json")
    json.dump(data, open(path, "w"), indent=1)
    m.log(f"wrote {path}; checking against the chain")
    sys.exit(subprocess.run([sys.executable, os.path.join(HERE, "farmledge_conformance.py"),
                             path, "--rpc", record["rpc_url"]]).returncode)


if __name__ == "__main__":
    main()
