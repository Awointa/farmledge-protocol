#!/usr/bin/env python3
"""Check a Farmledge proof bundle against the Stellar chain, with nothing
but Python's standard library and a public Stellar RPC.

    python3 verify_proof.py bundle.json [--rpc URL]

Exit 0: the leaf was in the warehouse's anchored books for its day.
The bundle format is in docs/events.md ("Farmer proofs"). This file is
copied verbatim into that document; it uses no Farmledge code or API.
"""
import base64, hashlib, json, struct, sys, urllib.request

B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"

def strkey(s):  # G... account or C... contract -> (kind, 32 bytes)
    bits = "".join(format(B32.index(c), "05b") for c in s)
    raw = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits) - 7, 8))
    body, crc = raw[:-2], int.from_bytes(raw[-2:], "little")
    c = 0
    for byte in body:  # CRC16-XModem
        c ^= byte << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x1021 if c & 0x8000 else c << 1) & 0xFFFF
    kind = {6 << 3: "account", 2 << 3: "contract"}.get(body[0])
    if c != crc or kind is None or len(body) != 33:
        sys.exit(f"bad address {s}")
    return kind, body[1:]

# --- XDR, just the ScVal types a leaf and its keys use ---------------------
u32 = lambda n: struct.pack(">I", n)
u64 = lambda n: struct.pack(">Q", n)
opaque = lambda b: u32(len(b)) + b + b"\0" * (-len(b) % 4)

def address(s):
    kind, key = strkey(s)
    return u32(0) + u32(0) + key if kind == "account" else u32(1) + key

def sv(t, v):  # ScVal
    if t == "u32": return u32(3) + u32(v)
    if t == "u64": return u32(5) + u64(v)
    if t == "bytes": return u32(13) + opaque(v)
    if t == "string": return u32(14) + opaque(v.encode())
    if t == "symbol": return u32(15) + opaque(v.encode())
    if t == "address": return u32(18) + address(v)
    if t == "vec": return u32(16) + u32(1) + u32(len(v)) + b"".join(v)
    if t == "map":  # keys sorted, as the host requires
        items = sorted(v.items())
        return u32(17) + u32(1) + u32(len(items)) + b"".join(sv("symbol", k) + x for k, x in items)

LEAF = {"version": "u32", "kind": "u32", "warehouse_id": "symbol", "day": "u32",
        "index": "u32", "deposit_no": "u64", "deposit_id": "string", "receipt_id": "u64",
        "receipt_version": "u32", "owner": "address", "commodity": "symbol",
        "grade": "symbol", "kg": "u64", "bags": "u32", "moisture_bps": "u32",
        "custodian": "address", "verifier": "address", "deposited_at": "u64",
        "at": "u64", "salt": "bytes"}

def leaf_xdr(leaf):
    f = {k: sv(t, bytes.fromhex(leaf[k]) if t == "bytes" else leaf[k]) for k, t in LEAF.items()}
    f["weight_source"] = sv("vec", [sv("symbol", leaf["weight_source"])])
    return sv("map", f)

def sha(*parts): return hashlib.sha256(b"".join(parts)).digest()

def leaf_hash(leaf): return sha(b"\x00", leaf_xdr(leaf))

def fold(h, index, proof):
    for sib in proof:
        h = sha(b"\x01", h, sib) if index % 2 == 0 else sha(b"\x01", sib, h)
        index //= 2
    return h

def chain(prev, day, leaves, root): return sha(prev, u32(day), u32(leaves), root)

# --- reading two entries from the chain -----------------------------------
def entry(rpc, contract, key, durability):  # -> value ScVal bytes, or None
    lk = u32(6) + address(contract) + key + u32(durability)
    req = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1,
        "method": "getLedgerEntries", "params": {"keys": [base64.b64encode(lk).decode()]}}).encode(),
        {"content-type": "application/json", "user-agent": "verify-proof/1"})
    found = json.load(urllib.request.urlopen(req, timeout=30))["result"].get("entries") or []
    if not found: return None
    data = base64.b64decode(found[0]["xdr"])
    # LedgerEntryData: type, ext, contract, key, durability, then the value.
    prefix = u32(6) + u32(0) + address(contract) + key + u32(durability)
    assert data.startswith(prefix), "unexpected entry layout"
    return data[len(prefix):]

def vec_of(val):  # a Vec of Bytes / U32 values -> python list
    assert val[:8] == u32(16) + u32(1)
    n, out, i = struct.unpack(">I", val[8:12])[0], [], 12
    for _ in range(n):
        t = struct.unpack(">I", val[i:i + 4])[0]
        if t == 3: out.append(struct.unpack(">I", val[i + 4:i + 8])[0]); i += 8
        elif t == 13:
            ln = struct.unpack(">I", val[i + 4:i + 8])[0]
            out.append(val[i + 8:i + 8 + ln]); i += 8 + ln + (-ln % 4)
        else: raise ValueError(f"ScVal type {t}")
    return out

def main():
    b = json.load(open(sys.argv[1]))
    rpc = sys.argv[sys.argv.index("--rpc") + 1] if "--rpc" in sys.argv else b["rpc"]
    leaf, proof = b["leaf"], [bytes.fromhex(p) for p in b["proof"]]
    if len(proof) != (b["day_leaves"] - 1).bit_length() or leaf["index"] >= b["day_leaves"]:
        sys.exit("FAIL: the proof does not fit a tree of day_leaves leaves")
    root = fold(leaf_hash(leaf), leaf["index"], proof)
    wh = sv("symbol", leaf["warehouse_id"])
    print(f"leaf hash  {leaf_hash(leaf).hex()}\nroot       {root.hex()}")

    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "DayRoot"), wh, sv("u32", leaf["day"])]), 0)
    if stored is not None:
        on_chain_root, leaves = vec_of(stored)
        ok = (on_chain_root, leaves) == (root, b["day_leaves"])
        print(f"day root   {'matches' if ok else 'DIFFERS'} the root stored on-chain for day {leaf['day']}")
        if not ok: sys.exit(1)

    # The head commits to every anchored day: recompute it from this day on.
    head = chain(bytes.fromhex(b["prev_head"]), leaf["day"], b["day_leaves"], root)
    for d in b.get("later_days", []):
        head = chain(head, d["day"], d["leaves"], bytes.fromhex(d["root"]))
    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "Head"), wh]), 1)
    if stored is None: sys.exit("FAIL: the warehouse has no anchor on-chain")
    last_day, on_chain_head = vec_of(stored)
    ok = on_chain_head == head
    print(f"head       {'matches' if ok else 'DIFFERS from'} the head on-chain (last day {last_day})")
    if not ok:
        sys.exit("FAIL" + ("" if b.get("later_days") or last_day == leaf["day"] else
                           ": later_days is missing the anchors after this day"))
    print(f"OK: {leaf['kg']} kg, lot {leaf['deposit_no']}, kind {leaf['kind']}, "
          f"in {leaf['warehouse_id']}'s books on day {leaf['day']}")

if __name__ == "__main__":
    main()
