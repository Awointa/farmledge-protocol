#!/usr/bin/env python3
"""Backend conformance checks for the Farmledge receipt contract (data_version 3).

    python3 farmledge_conformance.py export.json [--rpc URL] [--as-of UNIX_SECONDS]

The contract cannot enforce nine rules; the backend must (README, "Contract
frozen"). This checks each of them, I1-I9, against one JSON export of the
backend's records (format in conformance/README.md), and with --rpc also
against the chain. Exit 0 only if every invariant holds.

Python 3.8+, standard library only. Hashing and XDR come from
scripts/verify_proof.py, the published standalone verifier.
"""
import base64
import json
import os
import struct
import sys
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
import verify_proof as vp  # noqa: E402

DAY = 86_400
KIND_DEPOSIT, KIND_DISPATCH, KIND_BALANCE = 0, 1, 2
LEDGERS_PER_DAY = 17_280
SYMBOL_OK = set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_")

INVARIANTS = {
    "I1": "Freshness: no dispatch of a lot between its mint's anchor cutoff and the mint's resolution",
    "I2": "Pre-claim gate: no mint before every lot held on an older contract is pre-claimed",
    "I3": "One lot, one deposit number: contiguous, never reused, one deposit leaf each",
    "I4": "Complete, honest daily books, each leaf with a fresh salt",
    "I5": "Every warehouse anchored daily, in order, no day skipped, none missed",
    "I6": "A minted lot leaves only through the contract's exit",
    "I7": "Every leaf's proof delivered to the farmer within 24 hours, and it verifies",
    "I8": "Every event archived, no ledger gaps, archive consistent with the chain",
    "I9": "Minted receipts kept alive; Exited and Revoked receipts never sent",
}


def cutoff(day):
    """The end of UTC day `day`: when its books close."""
    return (day + 1) * DAY


def day_of(t):
    return t // DAY


def leaf_order(leaf):
    return (leaf["kind"], leaf["deposit_no"], leaf["at"])


def merkle(hashes):
    """Root and per-index proofs of a tree padded with sha256(0x00)."""
    pad = vp.sha(b"\x00")
    level = list(hashes) + [pad] * ((1 << max(len(hashes) - 1, 0).bit_length()) - len(hashes))
    levels = [level]
    while len(levels[-1]) > 1:
        cur = levels[-1]
        levels.append([vp.sha(b"\x01", cur[i], cur[i + 1]) for i in range(0, len(cur), 2)])

    def proof(i):
        out = []
        for lv in levels[:-1]:
            out.append(lv[i ^ 1])
            i //= 2
        return out
    return levels[-1][0], proof


class Export:
    def __init__(self, data):
        self.d = data
        self.lots = {(l["warehouse_id"], l["deposit_no"]): l for l in data["lots"]}
        self.days = {}
        for day in data["days"]:
            self.days.setdefault(day["warehouse_id"], {})[day["day"]] = day
        self.events = data["indexer"]["events"]
        self.anchored = {}
        for e in self.events:
            if e["type"] == "anchored":
                self.anchored.setdefault(e["fields"]["warehouse_id"], []).append(e["fields"])
        self.minted_requests = [m for m in data["mint_requests"] if m["outcome"] == "minted"]

    def warehouses(self):
        return [w["warehouse_id"] for w in self.d["warehouses"]]

    def dispatches_of(self, wh, no):
        return [x for x in self.d["dispatches"] if (x["warehouse_id"], x["deposit_no"]) == (wh, no)]

    def mint_of(self, wh, no):
        return next((m for m in self.minted_requests
                     if (m["warehouse_id"], m["deposit_no"]) == (wh, no)), None)


# --- I1 ---------------------------------------------------------------------
def check_i1_freshness(x, as_of):
    """No dispatch of a lot is recorded between the cutoff of the anchored day
    its mint uses and the mint's resolution; the freeze precedes submission;
    nothing is left pending past its max_time."""
    out = []
    for m in x.d["mint_requests"]:
        key = (m["warehouse_id"], m["deposit_no"])
        if m.get("submitted_at") is not None and m["frozen_at"] > m["submitted_at"]:
            out.append(f"mint request {m['id']}: frozen after it was submitted")
        end = m.get("resolved_at")
        if end is None:
            if m.get("max_time") is not None and as_of > m["max_time"] + 3_600:
                out.append(f"mint request {m['id']}: still pending an hour past its max_time")
            end = as_of
        # A minted lot must not move from its anchor's cutoff on; any
        # request's lot must not move while frozen.
        start = m["frozen_at"]
        if m["outcome"] == "minted":
            start = min(start, cutoff(m["anchor_day"]))
        for d in x.dispatches_of(*key):
            if d["via"] == "off_chain" and start <= d["recorded_at"] <= end:
                out.append(f"lot {key}: dispatch {d['id']} recorded at {d['recorded_at']} while "
                           f"mint request {m['id']} was frozen or after its anchor's cutoff")
    return out


# --- I2 ---------------------------------------------------------------------
def check_i2_preclaim_gate(x, rpc=None):
    """The gate is live even with nothing to claim: no mint in a warehouse is
    submitted before that warehouse's pre-claim is recorded as verified
    (with an empty list when no older contract exists). When an older
    contract holds live lots, each is pre-claimed (on-chain, with --rpc)
    and none is minted here."""
    out = []
    pc = x.d.get("preclaim") or {}
    old = pc.get("old_contract")
    verified = pc.get("verified_at") or {}
    # Only the backend's own warehouses: the contract is shared with any
    # warehouse the admin registers (Testnet's cost runs pre-claim in their own).
    ours = set(x.warehouses())
    preclaimed = [e for e in x.events
                  if e["type"] == "preclaimed" and e["fields"]["warehouse_id"] in ours]
    for m in x.d["mint_requests"]:
        if m.get("submitted_at") is None:
            continue
        v = verified.get(m["warehouse_id"])
        if v is None or m["submitted_at"] < v:
            out.append(f"mint request {m['id']} in {m['warehouse_id']} submitted before its "
                       "pre-claim was verified")
    if old is None:
        if preclaimed:
            out.append(f"{len(preclaimed)} preclaimed event(s), but the export declares no "
                       "older contract: no pre-claim should exist")
        return out
    claimed = {(e["fields"]["warehouse_id"], e["fields"]["deposit_no"]) for e in preclaimed}
    for lot in old["live_lots"]:
        key = (lot["warehouse_id"], lot["deposit_no"])
        if key not in claimed:
            out.append(f"lot {key}, live on {old['contract_id'][:8]}…, has no preclaimed event")
        if rpc and not rpc.is_claimed(x.d["contract_id"], *key):
            out.append(f"lot {key} is not claimed on-chain")
        if x.mint_of(*key):
            out.append(f"lot {key} is live on the older contract and was minted here")
    return out


# --- I3 ---------------------------------------------------------------------
def check_i3_deposit_numbers(x):
    """Per warehouse, deposit numbers run 1..N with no gap or repeat, and
    each has exactly one deposit leaf, on its intake day."""
    out = []
    for wh in x.warehouses():
        nos = sorted(l["deposit_no"] for l in x.d["lots"] if l["warehouse_id"] == wh)
        if nos != list(range(1, len(nos) + 1)):
            out.append(f"{wh}: deposit numbers are not 1..{len(nos)} without gaps or repeats: {nos}")
        seen = {}
        for day in x.days.get(wh, {}).values():
            for leaf in day["leaves"]:
                if leaf["kind"] == KIND_DEPOSIT:
                    seen.setdefault(leaf["deposit_no"], []).append(day["day"])
        for no in nos:
            lot = x.lots[(wh, no)]
            want = day_of(lot["intake_at"])
            got = seen.get(no, [])
            if got != [want] and want in x.days.get(wh, {}):
                out.append(f"{wh} lot {no}: deposit leaves on days {got}, expected one on {want}")
        for no in seen:
            if (wh, no) not in x.lots:
                out.append(f"{wh}: deposit leaf for unknown deposit number {no}")
    return out


# --- I4 ---------------------------------------------------------------------
def expected_books(x, wh, day):
    """The leaves a day's books must hold, without day/index/salt/at."""
    want = []
    end = cutoff(day)
    for (w, no), lot in x.lots.items():
        if w != wh:
            continue
        if day_of(lot["intake_at"]) == day:
            want.append((KIND_DEPOSIT, no, lot["kg"]))
        for d in x.dispatches_of(wh, no):
            if day_of(d["recorded_at"]) == day:
                want.append((KIND_DISPATCH, no, d["kg"]))
        if lot["intake_at"] < end:
            left = lot["kg"] - sum(d["kg"] for d in x.dispatches_of(wh, no) if d["recorded_at"] < end)
            if left > 0:
                want.append((KIND_BALANCE, no, left))
    return sorted(want)


def check_i4_books(x, rpc=None):
    """Each anchored day's leaves are exactly the day's deposits, dispatches
    and end-of-day balances, match their lots, are in order, carry fresh
    salts, and hash to the root that was anchored."""
    out = []
    salts = {}
    anchored = {(a["warehouse_id"], a["day"]): a for w in x.anchored.values() for a in w}
    for wh, days in x.days.items():
        for d, day in sorted(days.items()):
            leaves = day["leaves"]
            got = sorted((l["kind"], l["deposit_no"], l["kg"]) for l in leaves)
            want = expected_books(x, wh, d)
            if got != want:
                out.append(f"{wh} day {d}: books {got} differ from the records {want}")
            if [leaf_order(l) for l in leaves] != sorted(leaf_order(l) for l in leaves):
                out.append(f"{wh} day {d}: leaves not ordered by (kind, deposit_no, at)")
            for i, leaf in enumerate(leaves):
                lot = x.lots.get((wh, leaf["deposit_no"]))
                if leaf["day"] != d or leaf["index"] != i or leaf["warehouse_id"] != wh:
                    out.append(f"{wh} day {d} leaf {i}: day/index/warehouse do not match its place")
                if lot is not None:
                    for f in ("deposit_id", "owner", "commodity", "grade", "weight_source",
                              "custodian", "verifier", "deposited_at"):
                        if leaf[f] != lot[f if f != "deposited_at" else "intake_at"]:
                            out.append(f"{wh} day {d} leaf {i}: {f} differs from lot {leaf['deposit_no']}")
                if not set(leaf["grade"]) <= SYMBOL_OK or not set(leaf["commodity"]) <= SYMBOL_OK:
                    out.append(f"{wh} day {d} leaf {i}: grade or commodity is not a valid Symbol")
                salt = leaf["salt"]
                if len(bytes.fromhex(salt)) != 32:
                    out.append(f"{wh} day {d} leaf {i}: salt is not 32 bytes")
                if salt in salts:
                    out.append(f"{wh} day {d} leaf {i}: salt reused from {salts[salt]}")
                salts[salt] = (wh, d, i)
            root, _ = merkle([vp.leaf_hash(l) for l in leaves])
            a = anchored.get((wh, d))
            if a is None:
                out.append(f"{wh} day {d}: exported books but no anchored event")
            elif (bytes.fromhex(a["root"]), a["leaves"]) != (root, len(leaves)):
                out.append(f"{wh} day {d}: leaves hash to {root.hex()[:12]}…, anchored root is "
                           f"{a['root'][:12]}…")
            if rpc and a is not None:
                stored = rpc.day_root(x.d["contract_id"], wh, d)
                if stored is not None and stored != (root, len(leaves)):
                    out.append(f"{wh} day {d}: on-chain day root differs from the exported books")
    return out


# --- I5 ---------------------------------------------------------------------
def check_i5_anchoring(x, as_of, rpc=None):
    """Anchored days run without a gap from each warehouse's first to its
    latest, the latest is not overdue (2 hours past cutoff), and the head
    chain recomputes to the head on-chain."""
    out = []
    for wh in x.warehouses():
        chain = sorted(x.anchored.get(wh, []), key=lambda a: a["day"])
        if not chain:
            out.append(f"{wh}: never anchored")
            continue
        days = [a["day"] for a in chain]
        missing = sorted(set(range(days[0], days[-1] + 1)) - set(days))
        if missing:
            out.append(f"{wh}: days {missing} were skipped")
        due = day_of(as_of) - 1  # the last day whose cutoff has passed
        if as_of >= cutoff(due) + 2 * 3_600 and days[-1] < due:
            out.append(f"{wh}: day {due} not anchored {(as_of - cutoff(due)) // 3_600} hours "
                       "after its cutoff (urgent)")
        head = b"\x00" * 32
        for a in chain:
            if bytes.fromhex(a["prev_head"]) != head:
                out.append(f"{wh} day {a['day']}: prev_head breaks the chain")
            head = vp.chain(head, a["day"], a["leaves"], bytes.fromhex(a["root"]))
            if head.hex() != a["head"]:
                out.append(f"{wh} day {a['day']}: head does not recompute")
                head = bytes.fromhex(a["head"])
        if rpc:
            on_chain = rpc.head(x.d["contract_id"], wh)
            if on_chain is None or on_chain[1] != head:
                out.append(f"{wh}: recomputed head differs from the head on-chain")
    return out


# --- I6 ---------------------------------------------------------------------
def check_i6_minted_exit_on_chain(x):
    """After a lot is minted, every dispatch of it is an on-chain exit, its
    balance leaves name the receipt, and their weight is the receipt's."""
    out = []
    exits = {}
    for e in x.events:
        if e["type"] == "exited":
            exits[e["fields"]["receipt_id"]] = e
    for m in x.minted_requests:
        key = (m["warehouse_id"], m["deposit_no"])
        for d in x.dispatches_of(*key):
            if d["recorded_at"] > m["resolved_at"] and d["via"] != "exit":
                out.append(f"lot {key}: dispatch {d['id']} after its mint was recorded off-chain")
            if d["via"] == "exit" and d.get("receipt_id") not in exits:
                out.append(f"lot {key}: dispatch {d['id']} claims an exit no exited event shows")
        for d, day in x.days.get(key[0], {}).items():
            if cutoff(d) <= m["resolved_at"]:
                continue
            for leaf in day["leaves"]:
                if leaf["deposit_no"] != key[1] or leaf["kind"] != KIND_BALANCE:
                    continue
                if (leaf["receipt_id"], leaf["receipt_version"]) != (m["receipt_id"], 3):
                    out.append(f"lot {key} day {d}: balance leaf does not name receipt {m['receipt_id']}")
                on_chain = receipt_weight_at(x, m["receipt_id"], cutoff(d))
                if on_chain is not None and leaf["kg"] != on_chain:
                    out.append(f"lot {key} day {d}: anchored balance {leaf['kg']} kg, receipt "
                               f"family holds {on_chain} kg: grain moved outside the contract")
    return out


def receipt_weight_at(x, root_id, t):
    """Live (not exited or revoked) weight of a minted receipt and all its
    splits at time t, from the event archive."""
    family, weight, dead = {root_id}, {}, set()
    for e in sorted(x.events, key=lambda e: e["id"]):
        f = e["fields"]
        if e.get("at", 0) > t:
            break
        if e["type"] == "minted" and f["receipt_id"] == root_id:
            weight[root_id] = f["weight_kg"]
        elif e["type"] == "split" and f["parent_id"] in family:
            family.add(f["taken_id"])
            weight[f["parent_id"]] = f["remainder_weight_kg"]
            weight[f["taken_id"]] = f["taken_weight_kg"]
        elif e["type"] in ("exited", "revoked") and f["receipt_id"] in family:
            dead.add(f["receipt_id"])
    if root_id not in weight:
        return None
    return sum(w for r, w in weight.items() if r not in dead)


# --- I7 ---------------------------------------------------------------------
def check_i7_proofs_delivered(x, rpc_url=None):
    """Every anchored leaf has a delivered bundle within 24 hours of its
    anchor, carrying that exact leaf, whose proof folds to the anchored
    root; with --rpc each bundle is run through verify_proof.py."""
    out = []
    anchored_at = {(a["warehouse_id"], a["day"]): a for w in x.anchored.values() for a in w}
    delivered = {}
    for dl in x.d["deliveries"]:
        delivered.setdefault((dl["warehouse_id"], dl["day"], dl["index"]), []).append(dl)
    for wh, days in x.days.items():
        for d, day in days.items():
            a = anchored_at.get((wh, d))
            if a is None:
                continue
            for i, leaf in enumerate(day["leaves"]):
                got = [dl for dl in delivered.get((wh, d, i), []) if dl.get("delivered_at")]
                if not got:
                    out.append(f"{wh} day {d} leaf {i}: no bundle delivered")
                    continue
                dl = min(got, key=lambda g: g["delivered_at"])
                if dl["delivered_at"] > a["anchored_at"] + DAY:
                    out.append(f"{wh} day {d} leaf {i}: delivered "
                               f"{(dl['delivered_at'] - a['anchored_at']) // 3_600} h after its anchor")
                b = dl["bundle"]
                if b["leaf"] != leaf:
                    out.append(f"{wh} day {d} leaf {i}: bundle carries a different leaf")
                    continue
                proof = [bytes.fromhex(p) for p in b["proof"]]
                if vp.fold(vp.leaf_hash(leaf), leaf["index"], proof).hex() != a["root"]:
                    out.append(f"{wh} day {d} leaf {i}: bundle proof does not reach the anchored root")
                if rpc_url:
                    ok, msg = run_verifier(b, rpc_url)
                    if not ok:
                        out.append(f"{wh} day {d} leaf {i}: verify_proof.py rejects the bundle: {msg}")
    return out


def run_verifier(bundle, rpc_url):
    import subprocess
    import tempfile
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
        json.dump(bundle, fh)
    try:
        res = subprocess.run([sys.executable, os.path.join(os.path.dirname(vp.__file__),
                              "verify_proof.py"), fh.name, "--rpc", rpc_url],
                             capture_output=True, text=True)
    finally:
        os.unlink(fh.name)
    return res.returncode == 0, (res.stdout + res.stderr).strip().splitlines()[-1:]


# --- I8 ---------------------------------------------------------------------
def check_i8_indexer(x, rpc=None):
    """The archive covers start_ledger to the cursor without a gap, holds
    each event once, and reconciles: every receipt's weight is its minted
    or split-off weight less what was split from it."""
    out = []
    ix = x.d["indexer"]
    expect = ix["start_ledger"]
    for a, b in sorted(ix["ledger_ranges"]):
        if a > expect:
            out.append(f"indexer: ledgers {expect}..{a - 1} never indexed")
        expect = max(expect, b + 1)
    if expect <= ix["cursor"]:
        out.append(f"indexer: ledgers {expect}..{ix['cursor']} never indexed")
    ids = [e["id"] for e in x.events]
    if len(ids) != len(set(ids)):
        out.append("indexer: an event is archived twice")
    for e in x.events:
        if not ix["start_ledger"] <= e["ledger"] <= ix["cursor"]:
            out.append(f"indexer: event at ledger {e['ledger']} outside the indexed range")
    initial, taken = {}, {}
    for e in x.events:
        f = e["fields"]
        if e["type"] == "minted":
            initial[f["receipt_id"]] = f["weight_kg"]
        elif e["type"] == "split":
            initial[f["taken_id"]] = f["taken_weight_kg"]
            taken[f["parent_id"]] = taken.get(f["parent_id"], 0) + f["taken_weight_kg"]
    for r in x.d["receipts"]:
        rid = r["receipt_id"]
        if rid not in initial:
            out.append(f"receipt {rid}: no minted or split event archived")
        elif initial[rid] - taken.get(rid, 0) != r["weight_kg"]:
            out.append(f"receipt {rid}: archive gives {initial[rid] - taken.get(rid, 0)} kg, "
                       f"projection has {r['weight_kg']}")
    if rpc:
        for problem in rpc.compare_events(x.d["contract_id"], ix, x.events):
            out.append(f"indexer: {problem}")
    return out


# --- I9 ---------------------------------------------------------------------
def check_i9_keepalive(x, rpc=None):
    """No keep-alive submission names a receipt that had already exited or
    been revoked; with --rpc, every live receipt has at least 14 days of TTL."""
    out = []
    ended = {}
    for e in x.events:
        f = e["fields"]
        if e["type"] == "exited":
            ended[f["receipt_id"]] = f["exit_date"]
        elif e["type"] == "revoked":
            ended[f["receipt_id"]] = f["revoked_at"]
    for s in x.d["keepalive_submissions"]:
        for rid in s["receipt_ids"]:
            if rid in ended and ended[rid] <= s["submitted_at"]:
                out.append(f"keep-alive at {s['submitted_at']} named receipt {rid}, finished at {ended[rid]}")
        if len(s["receipt_ids"]) > 25:
            out.append(f"keep-alive at {s['submitted_at']} named more than 25 receipts")
    if rpc:
        latest = rpc.latest_ledger()
        for r in x.d["receipts"]:
            if r["status"] in ("Active", "Locked", "Recovery"):
                live_until = rpc.receipt_live_until(x.d["contract_id"], r["receipt_id"])
                if live_until is None or live_until - latest < 14 * LEDGERS_PER_DAY:
                    out.append(f"receipt {r['receipt_id']}: under 14 days of TTL left")
    return out


# --- decoding events as RPC returns them -------------------------------------
B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"


def strkey(version, payload):
    body = bytes([version]) + payload
    crc = 0
    for byte in body:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021 if crc & 0x8000 else crc << 1) & 0xFFFF
    return base64.b32encode(body + crc.to_bytes(2, "little")).decode().rstrip("=")


def decode_scval(b, i=0):
    """(value, next offset) for the ScVal types this contract's events use."""
    t = struct.unpack(">I", b[i:i + 4])[0]
    i += 4
    if t == 0:  # bool
        return bool(struct.unpack(">I", b[i:i + 4])[0]), i + 4
    if t == 1:  # void
        return None, i
    if t == 3:
        return struct.unpack(">I", b[i:i + 4])[0], i + 4
    if t == 5:
        return struct.unpack(">Q", b[i:i + 8])[0], i + 8
    if t in (13, 14, 15):  # bytes, string, symbol
        n = struct.unpack(">I", b[i:i + 4])[0]
        raw = b[i + 4:i + 4 + n]
        i += 4 + n + (-n % 4)
        return (raw.hex() if t == 13 else raw.decode()), i
    if t == 16:  # vec
        if not struct.unpack(">I", b[i:i + 4])[0]:
            return None, i + 4
        n, i, out = struct.unpack(">I", b[i + 4:i + 8])[0], i + 8, []
        for _ in range(n):
            v, i = decode_scval(b, i)
            out.append(v)
        return out, i
    if t == 17:  # map
        if not struct.unpack(">I", b[i:i + 4])[0]:
            return None, i + 4
        n, i, out = struct.unpack(">I", b[i + 4:i + 8])[0], i + 8, {}
        for _ in range(n):
            k, i = decode_scval(b, i)
            v, i = decode_scval(b, i)
            out[k] = v
        return out, i
    if t == 18:  # address
        kind = struct.unpack(">I", b[i:i + 4])[0]
        if kind == 0:  # account: PublicKey(ed25519)
            return strkey(6 << 3, b[i + 8:i + 40]), i + 40
        return strkey(2 << 3, b[i + 4:i + 36]), i + 36
    raise ValueError(f"ScVal type {t}")


def topic_names():
    """Each event's topic field names, in order, from docs/events.md."""
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
    import check_events_doc
    doc = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs", "events.md")).read()
    return {name: [f for f, _, topic in fields if topic]
            for name, fields in check_events_doc.doc_events(doc).items()}


def archive_event(rpc_event, topics):
    """One getEvents result as an archive record, decoded by name."""
    vals = [decode_scval(base64.b64decode(t))[0] for t in rpc_event["topic"]]
    name = vals[0]
    fields = dict(decode_scval(base64.b64decode(rpc_event["value"]))[0] or {})
    for field, value in zip(topics[name], vals[1:]):
        fields[field] = value
    t = __import__("datetime").datetime.fromisoformat(
        rpc_event["ledgerClosedAt"].replace("Z", "+00:00")).timestamp()
    return {"id": rpc_event["id"], "ledger": rpc_event["ledger"], "tx": rpc_event["txHash"],
            "at": int(t), "type": name, "fields": fields}


# --- chain reads --------------------------------------------------------------
class Rpc:
    def __init__(self, url):
        self.url = url

    def call(self, method, params):
        req = urllib.request.Request(self.url, json.dumps(
            {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
            {"content-type": "application/json", "user-agent": "farmledge-conformance/1"})
        res = json.load(urllib.request.urlopen(req, timeout=30))
        if "error" in res:
            raise RuntimeError(res["error"])
        return res["result"]

    def entry(self, contract, key, durability):
        lk = vp.u32(6) + vp.address(contract) + key + vp.u32(durability)
        found = self.call("getLedgerEntries", {"keys": [base64.b64encode(lk).decode()]}).get("entries") or []
        if not found:
            return None, None
        data = base64.b64decode(found[0]["xdr"])
        prefix = vp.u32(6) + vp.u32(0) + vp.address(contract) + key + vp.u32(durability)
        return data[len(prefix):], found[0].get("liveUntilLedgerSeq")

    def head(self, contract, wh):
        val, _ = self.entry(contract, vp.sv("vec", [vp.sv("symbol", "Head"), vp.sv("symbol", wh)]), 1)
        return None if val is None else tuple(vp.vec_of(val))

    def day_root(self, contract, wh, day):
        key = vp.sv("vec", [vp.sv("symbol", "DayRoot"), vp.sv("symbol", wh), vp.sv("u32", day)])
        val, _ = self.entry(contract, key, 0)
        return None if val is None else tuple(vp.vec_of(val))

    def is_claimed(self, contract, wh, no):
        key = vp.sv("vec", [vp.sv("symbol", "Claimed"), vp.sv("symbol", wh), vp.sv("u64", no // 1024)])
        val, _ = self.entry(contract, key, 1)
        if val is None:
            return False
        bits = val[8:8 + 128]  # ScVal Bytes: type, length, then the 128 bytes
        bit = no % 1024
        return bool(bits[bit // 8] & (1 << (bit % 8)))

    def receipt_live_until(self, contract, rid):
        _, live = self.entry(contract, vp.sv("vec", [vp.sv("symbol", "Receipt"), vp.sv("u64", rid)]), 1)
        return live

    def latest_ledger(self):
        return self.call("getLatestLedger", {})["sequence"]

    def events(self, contract, start, end=None):
        """Every event of `contract` from ledger `start` (within retention),
        as raw getEvents records."""
        out, cursor = [], None
        while True:
            q = {"filters": [{"type": "contract", "contractIds": [contract]}],
                 "pagination": {"limit": 200}}
            if cursor:
                q["pagination"]["cursor"] = cursor
            else:
                q["startLedger"] = start
            res = self.call("getEvents", q)
            batch = [e for e in res.get("events", []) if end is None or e["ledger"] <= end]
            out += batch
            if len(batch) < len(res.get("events", [])) or not res.get("events"):
                return out
            cursor = res.get("cursor")

    def compare_events(self, contract, ix, archived):
        """Every event RPC still holds in the indexed range is archived."""
        latest = self.latest_ledger()
        start = max(ix["start_ledger"], latest - 7 * LEDGERS_PER_DAY + 1_000)
        if start > ix["cursor"]:
            return []
        have = {e["id"] for e in archived}
        problems, cursor = [], None
        while True:
            q = {"filters": [{"type": "contract", "contractIds": [contract]}],
                 "pagination": {"limit": 200}}
            if cursor:
                q["pagination"]["cursor"] = cursor
            else:
                q["startLedger"] = start
            res = self.call("getEvents", q)
            events = res.get("events", [])
            for ev in events:
                if ev["ledger"] > ix["cursor"]:
                    return problems
                if ev["id"] not in have:
                    problems.append(f"event {ev['id']} at ledger {ev['ledger']} is not archived")
            if not events:
                return problems
            cursor = res.get("cursor")


def check_all(data, as_of, rpc_url=None):
    x = Export(data)
    rpc = Rpc(rpc_url) if rpc_url else None
    return {
        "I1": check_i1_freshness(x, as_of),
        "I2": check_i2_preclaim_gate(x, rpc),
        "I3": check_i3_deposit_numbers(x),
        "I4": check_i4_books(x, rpc),
        "I5": check_i5_anchoring(x, as_of, rpc),
        "I6": check_i6_minted_exit_on_chain(x),
        "I7": check_i7_proofs_delivered(x, rpc_url),
        "I8": check_i8_indexer(x, rpc),
        "I9": check_i9_keepalive(x, rpc),
    }


def main():
    data = json.load(open(sys.argv[1]))
    arg = lambda name: sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None
    as_of = int(arg("--as-of") or data.get("as_of") or __import__("time").time())
    results = check_all(data, as_of, arg("--rpc"))
    for inv, problems in results.items():
        print(f"{'PASS' if not problems else 'FAIL'}  {inv}  {INVARIANTS[inv]}")
        for p in problems:
            print(f"        - {p}")
    sys.exit(0 if not any(results.values()) else 1)


if __name__ == "__main__":
    main()
