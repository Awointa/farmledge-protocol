#!/usr/bin/env python3
"""A synthetic backend export that satisfies every invariant, built the
way a correct backend would build it. SYNTHETIC TEST DATA: no real farmer,
lot or transaction.

    python3 conformance/example.py > conformance/example.export.json

One warehouse, four days, four lots. Lot 1 is part-dispatched off-chain;
lot 2 is minted from day 1's books and part-exited on-chain; lot 3 is
dispatched in full; lot 4 arrives on day 2. `build()` is what the
conformance self-tests start from and break, one invariant at a time.
"""
import base64
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from farmledge_conformance import DAY, cutoff, day_of, expected_books, leaf_order, merkle, vp, Export  # noqa: E402

CONTRACT = "CBW6TA5UJMXJY2VYPJV6WUVWNUP3PKZNPXFMC2CNVCWZDF3SKIT4UAIN"
WH = "WH_CONF_01"
D = 20_700  # first day


def account(n):
    """A deterministic, valid G... strkey (no real account behind it)."""
    body = bytes([6 << 3]) + hashlib.sha256(b"farmledge-conformance-%d" % n).digest()
    crc = 0
    for byte in body:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021 if crc & 0x8000 else crc << 1) & 0xFFFF
    return base64.b32encode(body + crc.to_bytes(2, "little")).decode().rstrip("=")


def at(day, hour, minute=0):
    return day * DAY + hour * 3_600 + minute * 60


FLAWS = {
    "I1": "no freeze: lot 2 is dispatched after day 1's cutoff, then minted from day 1's stale balance",
    "I2": "no gate: lot 2 is minted before the warehouse's pre-claim is verified",
    "I2-unlisted": "older contract: a lot live on it is never pre-claimed",
    "I2-old-minted": "older contract: a pre-claimed lot is minted here anyway",
    "I3": "numbering: lot 4 is given number 6, leaving a gap",
    "I4": "incomplete books: lot 3's balance is left out of day 1",
    "I4-salt": "salt reused: two leaves share a salt",
    "I5": "skipped day: day 2 is never anchored",
    "I5-missed": "missed anchor: day 4 is not anchored hours after its cutoff",
    "I6": "minted lot dispatched off-chain: lot 2's 500 kg leaves without an exit",
    "I7": "no delivery: day 3's bundles are never sent",
    "I7-late": "late delivery: day 0's bundles are sent 30 hours after the anchor",
    "I8": "indexer gap: ledgers 1,020-1,029 are never indexed",
    "I9": "keep-alive names an exited receipt",
}


OLD_CONTRACT = "CB3PY2P5ZJ5TQIUEAZRGXOYKXKAZZVAMXCRG25GRWLURTKMKNXKYFPEM"


def build(flaw=None, older_contract=False):
    """A valid export, or, with `flaw` (a key of FLAWS), the export a backend
    lacking that one enforcement would produce. `older_contract` adds a
    deployment that started beside an older contract: lot 3 is live on it
    and pre-claimed before any mint (the gate's dormant branch, exercised)."""
    older_contract = older_contract or flaw in ("I2-unlisted", "I2-old-minted")
    custodian, verifier = account(100), account(101)
    owners = {1: account(1), 2: account(2), 3: account(1), 4: account(3)}
    lots = []
    for no, (day, hour, kg) in {1: (D, 8, 2_000), 2: (D, 9, 1_500), 3: (D + 1, 10, 800),
                                4: (D + 2, 11, 1_000)}.items():
        number = 6 if (flaw == "I3" and no == 4) else no
        owners[number] = owners[no]
        no = number
        lots.append({"warehouse_id": WH, "deposit_no": no, "deposit_id": f"DEP-CONF-{no:04d}",
                     "owner": owners[no], "commodity": "MAIZE_WHITE", "grade": "GRADE_1",
                     "weight_source": "Scale", "bags": kg // 100, "moisture_bps": 1250,
                     "custodian": custodian, "verifier": verifier, "kg": kg,
                     "intake_at": at(day, hour)})
    dispatches = [
        {"id": "DSP-1", "warehouse_id": WH, "deposit_no": 1, "kg": 300,
         "recorded_at": at(D + 1, 14), "via": "off_chain"},
        {"id": "DSP-2", "warehouse_id": WH, "deposit_no": 2, "kg": 500,
         "recorded_at": at(D + 2, 15), **({"via": "off_chain"} if flaw == "I6" else
                                          {"via": "exit", "receipt_id": 2})},
        {"id": "DSP-3", "warehouse_id": WH, "deposit_no": 3, "kg": 800,
         "recorded_at": at(D + 3, 9), "via": "off_chain"},
    ]
    if flaw == "I1":
        dispatches.append({"id": "DSP-0", "warehouse_id": WH, "deposit_no": 2, "kg": 200,
                           "recorded_at": at(D + 2, 0, 45), "via": "off_chain"})
    # Lot 2 is minted from day D+1's balance, after D+1 is anchored.
    mint = {"id": "MINT-1", "warehouse_id": WH, "deposit_no": 2, "anchor_day": D + 1,
            "frozen_at": at(D + 2, 1), "submitted_at": at(D + 2, 1, 1),
            "max_time": at(D + 2, 1, 6), "resolved_at": at(D + 2, 1, 2),
            "outcome": "minted", "receipt_id": 1}
    data = {"format": 1, "contract_id": CONTRACT, "network": "testnet",
            "as_of": cutoff(D + 3) + 3_600,
            "warehouses": [{"warehouse_id": WH}], "lots": lots, "dispatches": dispatches,
            "mint_requests": [mint],
            "preclaim": {"old_contract": None,
                         "verified_at": {WH: at(D + 2, 2) if flaw == "I2" else at(D, 0)}},
            "days": [], "deliveries": [], "keepalive_submissions": [],
            "receipts": [], "indexer": {"start_ledger": 1_000, "events": []}}
    x = Export(data)

    events, ledger = [], [1_000]

    def emit(kind, t, **fields):
        ledger[0] += 7
        events.append({"id": f"{ledger[0]:019d}-{len(events):010d}", "ledger": ledger[0],
                       "tx": hashlib.sha256(b"%d" % len(events)).hexdigest(), "at": t,
                       "type": kind, "fields": fields})

    head, salt = b"\x00" * 32, 0
    anchored_days = []
    for d in range(D, D + 4):
        if flaw == "I5" and d == D + 2:
            continue
        leaves = []
        for kind, no, kg in expected_books(x, WH, d):
            if flaw == "I4" and (d, kind, no) == (D + 1, 2, 3):
                continue
            lot = x.lots[(WH, no)]
            when = {0: lot["intake_at"], 2: cutoff(d)}.get(kind)
            if kind == 1:
                when = next(z["recorded_at"] for z in dispatches
                            if z["deposit_no"] == no and day_of(z["recorded_at"]) == d)
            governed = mint["deposit_no"] == no and mint["resolved_at"] < cutoff(d)
            salt += 1
            leaves.append({"version": 1, "kind": kind, "warehouse_id": WH, "day": d, "index": 0,
                           "deposit_no": no, "deposit_id": lot["deposit_id"],
                           "receipt_id": mint["receipt_id"] if governed else 0,
                           "receipt_version": 3 if governed else 0, "owner": lot["owner"],
                           "commodity": lot["commodity"], "grade": lot["grade"], "kg": kg,
                           "weight_source": lot["weight_source"], "bags": max(kg // 100, 1),
                           "moisture_bps": lot["moisture_bps"], "custodian": lot["custodian"],
                           "verifier": lot["verifier"], "deposited_at": lot["intake_at"],
                           "at": when, "salt": hashlib.sha256(
                               b"salt%d" % (1 if flaw == "I4-salt" and salt == 2 else salt)).hexdigest()})
        leaves.sort(key=leaf_order)
        for i, leaf in enumerate(leaves):
            leaf["index"] = i
        root, proof = merkle([vp.leaf_hash(l) for l in leaves])
        prev, head = head, vp.chain(head, d, len(leaves), root)
        anchored_at = cutoff(d) + 1_800
        emit("anchored", anchored_at, warehouse_id=WH, day=d, leaves=len(leaves), root=root.hex(),
             prev_head=prev.hex(), head=head.hex(), custodian=custodian, anchored_at=anchored_at)
        data["days"].append({"warehouse_id": WH, "day": d, "leaves": leaves})
        anchored_days.append((d, len(leaves), root, prev, proof, leaves, anchored_at))
        if d == D + 1:
            minted_leaf = next(l for l in leaves if l["deposit_no"] == 2 and l["kind"] == 2)
    if older_contract:
        live = 2 if flaw == "I2-old-minted" else 3
        data["preclaim"]["old_contract"] = {
            "contract_id": OLD_CONTRACT,
            "live_lots": [{"warehouse_id": WH, "deposit_no": live, "receipt_id": 7}]}
        if flaw != "I2-unlisted":
            emit("preclaimed", at(D, 0), warehouse_id=WH, deposit_no=live, claimed_at=at(D, 0))
    # The minted lot's on-chain life, during day D+2.
    emit("minted", mint["resolved_at"], receipt_id=1, warehouse_id=WH, deposit_no=2,
         weight_kg=minted_leaf["kg"], leaf_hash=vp.leaf_hash(minted_leaf).hex(), anchor_day=D + 1)
    if flaw != "I6":
        emit("split", at(D + 2, 15), parent_id=1, taken_id=2, taken_weight_kg=500,
             remainder_weight_kg=1_000)
        emit("exited", at(D + 2, 15), receipt_id=2, warehouse_id=WH, weight_kg=500,
             exit_date=at(D + 2, 15))
    # Events must sit in ledger order: re-sort by time and renumber.
    events.sort(key=lambda e: e["at"])
    for n, e in enumerate(events):
        e["ledger"] = 1_000 + 7 * (n + 1)
        e["id"] = f"{e['ledger']:019d}-{n:010d}"
    data["indexer"]["events"] = events
    data["indexer"]["cursor"] = events[-1]["ledger"] + 10
    data["indexer"]["ledger_ranges"] = [[1_000, data["indexer"]["cursor"]]]
    if flaw == "I8":
        data["indexer"]["ledger_ranges"] = [[1_000, 1_019], [1_030, data["indexer"]["cursor"]]]
    if flaw == "I5-missed":
        data["as_of"] = cutoff(D + 4) + 3 * 3_600

    for n, (d, count, root, prev, proof, leaves, anchored_at) in enumerate(anchored_days):
        later = [{"day": dd, "leaves": cc, "root": rr.hex()} for dd, cc, rr, *_ in anchored_days[n + 1:]]
        if flaw == "I7" and d == D + 3:
            continue
        for i, leaf in enumerate(leaves):
            data["deliveries"].append({
                "warehouse_id": WH, "day": d, "index": i, "channel": "sms",
                "delivered_at": anchored_at + (30 * 3_600 if flaw == "I7-late" and d == D else 600),
                "bundle": {"rpc": "https://soroban-testnet.stellar.org", "contract_id": CONTRACT,
                           "leaf": leaf, "proof": [p.hex() for p in proof(i)],
                           "day_leaves": count, "prev_head": prev.hex(), "later_days": later}})
    data["keepalive_submissions"] = [{"submitted_at": at(D + 3, 12),
                                      "receipt_ids": [1, 2] if flaw == "I9" else [1]}]
    data["receipts"] = [
        {"receipt_id": 1, "status": "Active", "weight_kg": 1_000, "deposit_no": 2, "warehouse_id": WH},
        {"receipt_id": 2, "status": "Exited", "weight_kg": 500, "deposit_no": 2, "warehouse_id": WH},
    ]
    if flaw == "I6":
        data["receipts"] = [{"receipt_id": 1, "status": "Active", "weight_kg": 1_500,
                             "deposit_no": 2, "warehouse_id": WH}]
    return data


if __name__ == "__main__":
    json.dump(build(sys.argv[1] if len(sys.argv) > 1 else None), sys.stdout, indent=1)
