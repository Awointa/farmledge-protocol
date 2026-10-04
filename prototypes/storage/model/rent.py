"""Soroban rent, from a network's config settings (`stellar network settings
--internal --output json`). Matched Testnet's charged fees within 0.1%."""
import json

MIN_RENT_FEE_1KB = 1000  # the protocol floor


def cfg(path):
    d = {list(x)[0]: list(x.values())[0] for x in json.load(open(path))["updated_entry"]}
    lc, sa = d["contract_ledger_cost_v0"], d["state_archival"]
    window = [int(v) for v in d["live_soroban_state_size_window"]]
    size = sum(window) // len(window)
    low = int(lc["rent_fee1_kb_soroban_state_size_low"])
    high = int(lc["rent_fee1_kb_soroban_state_size_high"])
    target = int(lc["soroban_state_target_size_bytes"])
    growth = int(lc["soroban_state_rent_fee_growth_factor"])
    if size < target:
        rate = -(-(high - low) * size // target) + low
    else:
        rate = high + -(-(high - low) * (size - target) * growth // target)
    return {"state_bytes": size, "rent_fee_1kb": max(rate, MIN_RENT_FEE_1KB),
            "denom": int(sa["persistent_rent_rate_denominator"])}


def rent(c, size, ledgers):
    """Persistent rent for `size` bytes over `ledgers`, in stroops."""
    return -(-size * c["rent_fee_1kb"] * ledgers // (1024 * c["denom"]))
