#!/usr/bin/env python3
"""Measures the THROWAWAY storage-strategy prototype on Testnet.

    python3 prototypes/storage/measure_proto.py

Needs a prototype deployed with build.sh and recorded in
results/prototype-contract.json. Submits real transactions, reads the charged
fees back from RPC and entry sizes with getLedgerEntries. Writes
results/proto.costs.json and results/proto.entries.json, never deployments/.
"""
import hashlib
import json
import os
import secrets
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "scripts"))
import measure_costs as m  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
net = m.Net(json.load(open(os.path.join(HERE, "results", "prototype-contract.json"))))
net.network = "testnet"
rows, entries = [], {}
ids = {r: f"fl-{r}" for r in ["custodian", "verifier", "farmer", "lender", "buyer"]}
A = {r: net.addr(i) for r, i in ids.items()}
WH, COM, GR = "WH_KANO_01", "MAIZE_WHITE", "GRADE_1"
run = int(time.time())


def save():
    json.dump(rows, open(os.path.join(HERE, "results", "proto.costs.json"), "w"), indent=2)
    json.dump(entries, open(os.path.join(HERE, "results", "proto.entries.json"), "w"), indent=2)


def op(label, source, fn, **kw):
    m.log(label)
    out, h = net.invoke(source, fn, **kw)
    rows.append(m.costs(net, label, h))
    save()
    return out


def size(label, *key_parts):
    b, _ = net.entry_size(m.scval_key(*key_parts))
    entries[label] = b
    save()


def admin(fn, **kw):
    args = ["stellar", "contract", "invoke", "--id", net.c, "--source-account",
            "farmledge-admin", "--network", "testnet", "--", fn]
    for k, v in kw.items():
        args += [f"--{k}", v]
    m.sh(*args, check=False)


def sim(fn, **kw):
    args = ["stellar", "contract", "invoke", "--id", net.c, "--source-account",
            ids["buyer"], "--network", "testnet", "--send=no", "--", fn]
    for k, v in kw.items():
        args += [f"--{k}", str(v)]
    return m.sh(*args).stdout.strip()


def node(l, r):
    return hashlib.sha256(b"\x01" + l + r).digest()


def full_tree(leaves):
    """Root and a proof for every index of a complete tree."""
    levels = [leaves]
    while len(levels[-1]) > 1:
        cur = levels[-1]
        levels.append([node(cur[i], cur[i + 1]) for i in range(0, len(cur), 2)])

    def proof(index):
        out = []
        for level in levels[:-1]:
            out.append(level[index ^ 1])
            index //= 2
        return out
    return levels[-1][0], proof


m.log("registering roles")
admin("add_warehouse", warehouse_id=WH)
admin("add_custodian", warehouse_id=WH, custodian=A["custodian"])
admin("add_verifier", verifier=A["verifier"])

# --- A: mint on demand ---------------------------------------------------------
base = (run % 100_000) * 4096
for n, label in [(0, "A a_mint — first in its bitmap chunk (creates owner index)"),
                 (1, "A a_mint — later deposit, chunk exists")]:
    rid = op(label, ids["custodian"], "a_mint", custodian=ids["custodian"],
             verifier=ids["verifier"], deposit_id=f"DEP-{run}-{n}", deposit_no=base + n,
             farmer=ids["farmer"], warehouse_id=WH, commodity=COM, grade=GR,
             weight_kg=2000, weight_source='"Scale"', bag_count=20, moisture_pct=1250)
size("A receipt", "Receipt", {"u64": rid.strip('"')})
size("A bitmap chunk (1,024 deposits)", "Minted", {"symbol": WH}, {"u64": str(base // 1024)})

# --- B: daily anchoring ----------------------------------------------------------
day0 = 20_000 + (run % 5_000)
DEPTH = 11  # 2,048 balance leaves: a warehouse with ~2,000 active lots
leaf = {
    "version": 1, "kind": 2, "warehouse_id": WH, "day": day0, "index": 5,
    "deposit_id": f"DEP-{run}-D", "owner": A["farmer"], "commodity": COM, "grade": GR,
    "kg": 1500, "bags": 15, "moisture_bps": 1250, "custodian": A["custodian"],
    "verifier": A["verifier"], "at": run, "salt": secrets.token_hex(32),
}
leaf2 = dict(leaf, index=6, salt=secrets.token_hex(32), kg=800, deposit_id=f"DEP-{run}-E")
lh = bytes.fromhex(sim("leaf_hash", leaf=json.dumps(leaf)).strip('"'))
lh2 = bytes.fromhex(sim("leaf_hash", leaf=json.dumps(leaf2)).strip('"'))
# 2,048 balance leaves: a warehouse holding about 2,000 active lots. Only
# the two real leaves' paths matter to the cost; the rest are random.
leaves = [secrets.token_bytes(32) for _ in range(2 ** DEPTH)]
leaves[5], leaves[6] = lh, lh2
root, proof_of = full_tree(leaves)
op("B b_anchor — first day, head + day root stored", ids["custodian"], "b_anchor",
   custodian=ids["custodian"], warehouse_id=WH, day=day0, root=root.hex(), leaves=2048,
   keep_day="true")
op("B b_anchor — next day, head + day root stored", ids["custodian"], "b_anchor",
   custodian=ids["custodian"], warehouse_id=WH, day=day0 + 1,
   root=secrets.token_hex(32), leaves=2048, keep_day="true")
op("B b_anchor — head only (day root in the event)", ids["custodian"], "b_anchor",
   custodian=ids["custodian"], warehouse_id=WH, day=day0 + 2,
   root=secrets.token_hex(32), leaves=2048, keep_day="false")
size("B day root", "DayRoot", {"symbol": WH}, {"u32": day0})
size("B head", "Head", {"symbol": WH})

# --- D: pledge from an anchored leaf ---------------------------------------------
pid = op(f"D d_pledge — proof depth {DEPTH}, first pledge (creates leaf bitmap, lender index)",
         ids["farmer"], "d_pledge", lender=ids["lender"], leaf=json.dumps(leaf),
         proof=json.dumps([p.hex() for p in proof_of(5)]), loan_ref="LOAN-D1",
         due_date=run + 10_000, grace_period=0)
size("D pledge record", "Pledge", {"u64": pid.strip('"')})
size("D leaf bitmap (1,024 leaves)", "LeafUsed", {"symbol": WH}, {"u32": day0}, {"u32": 0})
op(f"D d_pledge — proof depth {DEPTH}, later pledge", ids["farmer"], "d_pledge",
   lender=ids["lender"], leaf=json.dumps(leaf2),
   proof=json.dumps([p.hex() for p in proof_of(6)]), loan_ref="LOAN-D2",
   due_date=run + 10_000, grace_period=0)
op("D d_release", ids["lender"], "d_release", pledge_id=pid.strip('"'))

# --- C: aggregated lots --------------------------------------------------------
cbase = base + 2048
for n in range(10):
    label = f"C c_add_lot — lot {n + 1} into the farmer's entry"
    op(label, ids["custodian"], "c_add_lot", custodian=ids["custodian"],
             verifier=ids["verifier"], farmer=ids["farmer"], warehouse_id=WH,
             commodity=COM, deposit_no=cbase + n, grade=GR, weight_kg=1000,
             bag_count=10, moisture_pct=1250)
    if n + 1 in (1, 3, 10):
        size(f"C aggregate entry, {n + 1} lot(s)", "Agg", {"address": A["farmer"]},
             {"symbol": WH}, {"symbol": COM})
op("C c_pledge_lot — one lot of 10", ids["farmer"], "c_pledge_lot", farmer=ids["farmer"],
   lender=ids["lender"], warehouse_id=WH, commodity=COM, deposit_no=cbase + 3,
   loan_ref="LOAN-C1", due_date=run + 10_000, grace_period=0)
size("C aggregate entry, 10 lots, one pledged", "Agg", {"address": A["farmer"]},
     {"symbol": WH}, {"symbol": COM})
save()
m.log("done")
