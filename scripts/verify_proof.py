#!/usr/bin/env python3
"""Check a Farmledge proof bundle against the Stellar chain, with nothing
but Python's standard library and a public Stellar RPC.

    python3 verify_proof.py bundle.json [--rpc URL]

The last line printed is always `RESULT <CODE>`; the codes are in OUTCOMES
below. Exit 0: the leaf was in the warehouse's anchored books for its day.
The bundle format is in docs/events.md ("Farmer proofs"). This file is
copied verbatim into that document; it uses no Farmledge code or API.
"""
import base64, hashlib, json, struct, sys, urllib.request

# code: (exit status, what it means). Exit 0 is the only "verified".
OUTCOMES = {
    "OK": (0, "the root is on-chain for the leaf's day, and the head chain reaches the head on-chain"),
    "DAY_ROOT_EXPIRED": (0, "the day's root has left the chain (after 30 days, as designed); verified through the head chain alone"),
    "NOT_YET_ANCHORED": (2, "the warehouse has not anchored the leaf's day yet: not a failure, check again after the next anchor"),
    "PROOF_DOES_NOT_FIT": (1, "the proof does not fit a tree of day_leaves leaves"),
    "DAY_IN_FUTURE": (1, "the leaf's day is after the chain's current day, so it can never be anchored"),
    "DAY_ROOT_DIFFERS": (1, "the root stored on-chain for the leaf's day is not the root this proof gives"),
    "NO_HEAD": (1, "the warehouse has never anchored anything on-chain"),
    "HEAD_MISMATCH": (1, "the head chain from the leaf's day does not reach the head on-chain"),
    "BAD_BUNDLE": (4, "the bundle is not a well-formed proof bundle"),
    "COULD_NOT_CHECK": (3, "the chain could not be read: this is no result at all"),
}

class Outcome(Exception): pass

def finish(code, detail=""):
    print(f"{code}: {OUTCOMES[code][1]}" + (f" ({detail})" if detail else ""))
    print(f"RESULT {code}")
    sys.exit(OUTCOMES[code][0])

B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"

def strkey(s):  # G... account or C... contract -> (kind, 32 bytes)
    if len(s) != 56: raise ValueError(f"bad address {s}")  # 35 bytes exactly: one spelling per key
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
        raise ValueError(f"bad address {s}")
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

# --- reading the chain: any failure here is COULD_NOT_CHECK, never a result
def rpc_call(rpc, method, params):
    req = urllib.request.Request(rpc, json.dumps({"jsonrpc": "2.0", "id": 1,
        "method": method, "params": params}).encode(),
        {"content-type": "application/json", "user-agent": "verify-proof/2"})
    try:
        reply = json.load(urllib.request.urlopen(req, timeout=30))
        if "error" in reply: raise ValueError(reply["error"])
        return reply["result"]
    except Exception as e:
        raise Outcome("COULD_NOT_CHECK", f"{method}: {e}")

def entry(rpc, contract, key, durability):  # -> (value ScVal bytes, ledger written), or None
    lk = u32(6) + address(contract) + key + u32(durability)
    found = rpc_call(rpc, "getLedgerEntries", {"keys": [base64.b64encode(lk).decode()]}).get("entries") or []
    if not found: return None
    # An archived entry is still returned, with its value: archived is not absent.
    data = base64.b64decode(found[0]["xdr"])
    # LedgerEntryData: type, ext, contract, key, durability, then the value.
    prefix = u32(6) + u32(0) + address(contract) + key + u32(durability)
    if not data.startswith(prefix): raise Outcome("COULD_NOT_CHECK", "unexpected entry layout")
    return data[len(prefix):], found[0].get("lastModifiedLedgerSeq")

def vec_of(val, *types):  # exactly a Vec of these: "u32", or "bytes" of 32 -> python list
    if val[:8] != u32(16) + u32(1) or struct.unpack(">I", val[8:12])[0] != len(types):
        raise Outcome("COULD_NOT_CHECK", "unexpected value type")
    out, i = [], 12
    for t in types:
        tag = struct.unpack(">I", val[i:i + 4])[0]
        if t == "u32" and tag == 3: out.append(struct.unpack(">I", val[i + 4:i + 8])[0]); i += 8
        elif t == "bytes" and tag == 13 and val[i + 4:i + 8] == u32(32) and len(val) >= i + 40:
            out.append(val[i + 8:i + 40]); i += 40
        else: raise Outcome("COULD_NOT_CHECK", f"unexpected value at byte {i}")
    if i != len(val): raise Outcome("COULD_NOT_CHECK", "unexpected bytes after the value")
    return out

def check(b, rpc):
    try:
        if not (isinstance(b, dict) and isinstance(b["leaf"], dict) and isinstance(b["proof"], list)
                and isinstance(b.get("later_days", []), list)): raise ValueError("not a bundle's shape")
        leaf, proof = b["leaf"], [bytes.fromhex(p) for p in b["proof"]]
        day, n, idx = leaf["day"], b["day_leaves"], leaf["index"]
        ints = [day, n, idx] + [leaf[k] for k, t in LEAF.items() if t in ("u32", "u64")]
        ints += [x for d in b.get("later_days", []) for x in (d["day"], d["leaves"])]
        if not all(type(x) is int for x in ints): raise ValueError("integer fields must be JSON integers")
        h = leaf_hash(leaf)
        prev = bytes.fromhex(b["prev_head"])
        later = [(d["day"], d["leaves"], bytes.fromhex(d["root"])) for d in b.get("later_days", [])]
        for d in later: chain(prev, *d)  # rejects days or counts that are not u32
        if any(len(x) != 32 for x in proof + [prev] + [d[2] for d in later]): raise ValueError("hashes are 32 bytes")
        wh = sv("symbol", leaf["warehouse_id"])
        address(b["contract_id"])
    except Exception as e:
        raise Outcome("BAD_BUNDLE", f"{type(e).__name__}: {e}")
    if not 1 <= n <= 2 ** 20 or len(proof) != (n - 1).bit_length() or not 0 <= idx < n:
        raise Outcome("PROOF_DOES_NOT_FIT", f"{len(proof)} siblings, index {idx}, {n} leaves")
    root = fold(h, idx, proof)
    print(f"leaf hash  {h.hex()}\nroot       {root.hex()}")

    # The latest anchored day is in the warehouse's head; today is the chain's.
    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "Head"), wh]), 1)
    if stored is None: raise Outcome("NO_HEAD", leaf["warehouse_id"])
    last_day, on_chain_head = vec_of(stored[0], "u32", "bytes")
    today = int(rpc_call(rpc, "getLatestLedger", {})["closeTime"]) // 86400
    print(f"head       last anchored day {last_day}; chain's day now {today}")
    if day > today: raise Outcome("DAY_IN_FUTURE", f"day {day}")
    if day > last_day: raise Outcome("NOT_YET_ANCHORED", f"day {day}, last anchored {last_day}")

    stored = entry(rpc, b["contract_id"], sv("vec", [sv("symbol", "DayRoot"), wh, sv("u32", day)]), 0)
    if stored is not None:
        on_chain_root, leaves = vec_of(stored[0], "bytes", "u32")
        if (on_chain_root, leaves) != (root, n):
            raise Outcome("DAY_ROOT_DIFFERS", f"day {day}: on-chain {on_chain_root.hex()}, {leaves} leaves")
        print(f"day root   matches the root stored on-chain for day {day} (written in ledger {stored[1]})")

    # The head commits to every anchored day: recompute it from this day on.
    head = chain(prev, day, n, root)
    for d in later: head = chain(head, *d)
    if head != on_chain_head:
        raise Outcome("HEAD_MISMATCH", "later_days is missing the anchors after this day"
                      if not later and last_day != day else f"on-chain head {on_chain_head.hex()}")
    print(f"head       matches the head on-chain (last day {last_day})")
    print(f"{leaf['kg']} kg, lot {leaf['deposit_no']}, kind {leaf['kind']}, "
          f"in {leaf['warehouse_id']}'s books on day {day}")
    return "OK" if stored is not None else "DAY_ROOT_EXPIRED"

def main():
    try:
        b = json.load(open(sys.argv[1]))
        rpc = sys.argv[sys.argv.index("--rpc") + 1] if "--rpc" in sys.argv else b["rpc"]
    except Exception as e:
        finish("BAD_BUNDLE", f"{type(e).__name__}: {e}")
    # The RPC is the one thing trusted. A bundle could name a server that lies.
    print(f"rpc        {rpc}" + ("" if "--rpc" in sys.argv else
          "  (named by the bundle: pass --rpc with an endpoint you chose)"))
    try:
        finish(check(b, rpc))
    except Outcome as o:
        finish(*o.args)
    except Exception as e:  # anything unforeseen in what the chain returned
        finish("COULD_NOT_CHECK", f"{type(e).__name__}: {e}")

if __name__ == "__main__":
    main()
