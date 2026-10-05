#!/usr/bin/env python3
"""The `data_version` 3 cost tables in docs/archival.md and
docs/storage-strategy.md, from what Testnet actually charged.

    scripts/cost_model.py

Inputs: deployments/testnet.costs.json and testnet.entries.json (written by
scripts/measure_costs.py) and the network settings snapshots
deployments/{testnet,mainnet}.settings.<date>.json (`stellar network settings
--internal --output json`). Every measured fee's rent part is rescaled from
the Testnet rate it was charged at to each rate shown; the rest of the fee
does not depend on the rent rate. That rate is read off the run itself (the
rent charged for a native extend of a known size and length), because
Testnet's rate drifts during the day; the settings snapshot gives mainnet's.
"""
import json
from pathlib import Path

D = Path(__file__).resolve().parent.parent / "deployments"
SETTINGS_DATE = "2026-10-05"
X, DAY, TTL_FEE = 10_000_000, 17_280, 2_542  # stroops/XLM, ledgers/day, per-entry extend fee
FLOOR = 1_000

# --- assumptions (stated in the docs) ----------------------------------------
LIFE = 270            # days a deposit is in store
PLEDGED = 0.30        # share of deposits pledged, so minted
PLEDGE_AT = 30        # day of the pledge (and the mint); released 90 days later
PER_WH_MONTH = 50     # deposits per warehouse per month


def rate(path):
    """Rent rate (stroops per KB) from a settings snapshot, as the network
    computes it from the average live state size."""
    d = {list(x)[0]: list(x.values())[0] for x in json.load(open(path))["updated_entry"]}
    lc = d["contract_ledger_cost_v0"]
    window = [int(v) for v in d["live_soroban_state_size_window"]]
    size = sum(window) // len(window)
    low, high = int(lc["rent_fee1_kb_soroban_state_size_low"]), int(lc["rent_fee1_kb_soroban_state_size_high"])
    target, growth = int(lc["soroban_state_target_size_bytes"]), int(lc["soroban_state_rent_fee_growth_factor"])
    r = (-(-(high - low) * size // target) + low if size < target
         else high + -(-(high - low) * (size - target) * growth // target))
    return max(r, FLOOR), size


SNAP_RATE, T_STATE = rate(D / f"testnet.settings.{SETTINGS_DATE}.json")
M_RATE, M_STATE = rate(D / f"mainnet.settings.{SETTINGS_DATE}.json")
ROWS = {r["op"]: r for r in json.load(open(D / "testnet.costs.json"))}
SIZES = {k: v["bytes"] for k, v in json.load(open(D / "testnet.entries.json")).items()}


def charged_rate():
    """The Testnet rate the run was charged at, from the +5-day extend:
    rent = bytes x rate x ledgers / (1024 x 1215), solved for rate."""
    row = next(r for k, r in ROWS.items() if k.startswith("native extend — 1 receipt, +5 days"))
    e = row["extended"][0]
    return row["rent"] * 1024 * 1215 / (e["bytes"] * e["ledgers"])


T_RATE = charged_rate()
RATES = [("Testnet", T_RATE), ("mainnet today", M_RATE), ("mainnet at 10,000/KB", 10_000)]


def rent(r, size, ledgers, denom=1215):
    """Rent for `size` bytes over `ledgers` at rate `r`. Temporary storage
    uses denominator 2430."""
    return -(-size * r * ledgers // (1024 * denom))


def fee(op, r):
    row = ROWS[op]
    return (row["fee"] - row["rent"]) + row["rent"] * r / T_RATE


def op(prefix):
    return next(k for k in ROWS if k.startswith(prefix))


def renewals(life, threshold=30, extend_to=175):
    """Days of TTL bought after the write (which prepays 175 days), and
    renewals, for an entry living `life` days under the daily keep-alive."""
    live, n = 175, 0
    for day in range(1, life + 1):
        if live - day < threshold:
            live, n = day + extend_to, n + 1
    return live - 175, n


def keepalive(r, size, life):
    bought, n = renewals(life)
    batch = fee(op("extend_ttl_batch — 25"), r) / 25
    return rent(r, size, bought * DAY) + n * (TTL_FEE + batch)


def anchors_per_warehouse_year(r):
    head = SIZES["Head"]
    head_keepalive = rent(r, head, 365 * DAY) + 2 * (TTL_FEE + fee(op("extend_ttl_batch — 25"), r) / 25)
    return 365 * fee(op("anchor — next day"), r) + head_keepalive


def pledged_deposit(r):
    """Minted at the pledge, locked, released, exited; receipt kept alive
    from the mint to the exit."""
    return (fee(op("mint_from_anchor — later lot"), r) + fee(op("lock — full"), r)
            + fee(op("unlock — full"), r) + fee(op("exit — full"), r)
            + keepalive(r, SIZES["Receipt — Active, freshly minted"], LIFE - PLEDGE_AT))


def chunk_creation(r):
    """Creating one claim chunk: the two pre-claims differ only in that."""
    return fee(op("preclaim — 25 numbers, creates"), r) - fee(op("preclaim — 25 numbers, chunk exists"), r)


def chunk_share(r):
    """The claim chunk, shared by the 1,024 numbers it covers."""
    return chunk_creation(r) / 1024


def anchor_share(r):
    return anchors_per_warehouse_year(r) / 12 / PER_WH_MONTH


def per_deposit(r):
    return PLEDGED * pledged_deposit(r) + chunk_share(r) + anchor_share(r)


def check_rate():
    """The rent formula, at the charged rate, against an independent charged
    fee: the 25-receipt native extend."""
    row = ROWS[op("native extend — 25 receipts")]
    return sum(rent(T_RATE, e["bytes"], e["ledgers"]) for e in row["extended"]), row["rent"]


def main():
    predicted, charged = check_rate()
    print(f"Testnet charged {T_RATE:,.0f}/KB during the run (snapshot {SNAP_RATE}/KB, state "
          f"{T_STATE / 1e9:.2f} GB); mainnet {M_RATE}/KB on {SETTINGS_DATE} (state "
          f"{M_STATE / 1e9:.2f} GB). Rent formula check, 25-receipt extend: "
          f"{predicted:,} predicted vs {charged:,} charged.\n")

    print("| operation | Testnet | mainnet today | mainnet at 10,000/KB | of which rent (Testnet, stroops) |")
    print("|---|---:|---:|---:|---:|")
    for name, row in ROWS.items():
        cells = " | ".join(f"{fee(name, r) / X:.4f}" for _, r in RATES)
        print(f"| {name} | {cells} | {row['rent']:,} |")

    print("\n| entry | bytes |\n|---|---:|")
    for k, v in SIZES.items():
        print(f"| {k} | {v:,} |")

    print("\n| | " + " | ".join(n for n, _ in RATES) + " |\n|---|---:|---:|---:|")
    lines = [
        ("anchors, per warehouse per year", anchors_per_warehouse_year),
        ("a pledged deposit (mint, lock, unlock, exit, keep-alive)", pledged_deposit),
        ("an unpledged deposit (its share of anchors and of a claim chunk)",
         lambda r: chunk_share(r) + anchor_share(r)),
        ("**per deposit, 30% pledged**", per_deposit),
    ]
    for label, fn in lines:
        print(f"| {label} | " + " | ".join(f"{fn(r) / X:.4f}" for _, r in RATES) + " |")

    print("\n| rate | XLM per deposit | 200 deposits / month | 2,000 deposits / month |")
    print("|---|---:|---:|---:|")
    for name, r in RATES:
        v = per_deposit(r) / X
        print(f"| {name} | {v:.4f} | {200 * v:,.1f} | {2_000 * v:,.0f} |")

    print("\n| creating one claim chunk | " + " | ".join(f"{chunk_creation(r) / X:.4f}" for _, r in RATES) + " |")
    print("\n| pre-claim, one call | " + " | ".join(n for n, _ in RATES) + " |\n|---|---:|---:|---:|")
    for name in ROWS:
        if name.startswith("preclaim"):
            print(f"| {name} | " + " | ".join(f"{fee(name, r) / X:.4f}" for _, r in RATES) + " |")


if __name__ == "__main__":
    main()
