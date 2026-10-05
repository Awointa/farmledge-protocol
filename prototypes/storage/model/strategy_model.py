"""Cost model behind docs/storage-strategy.md.

    python3 prototypes/storage/model/strategy_model.py

Inputs, all measured on Testnet:
  deployments/testnet.v2.costs.json / v2.entries.json  the data_version 2 design
  prototypes/storage/results/proto.*.json           the throwaway prototype (A, B, C, D)
Rates: the network settings snapshots in prototypes/storage/data/.
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PROTO = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(PROTO))
sys.path.insert(0, HERE)
from rent import cfg, rent  # noqa: E402
from analyse_strat import strat  # noqa: E402

T = cfg(f"{PROTO}/data/testnet.settings.2026-10-04.json")
M = cfg(f"{PROTO}/data/mainnet.settings.2026-10-04.json")
HI = dict(M, rent_fee_1kb=10000)
RATES = [("Testnet", T), ("mainnet today", M), ("mainnet at 10,000/KB", HI)]
X, DAY, TTLW = 1e7, 17280, 2542

D = f"{REPO}/deployments"
CUR = {r["op"]: r for r in json.load(open(f"{D}/testnet.v2.costs.json"))}
CUR_E = json.load(open(f"{D}/testnet.v2.entries.json"))
PRO = {r["op"]: r for r in json.load(open(f"{PROTO}/results/proto.costs.json"))}
PRO_E = json.load(open(f"{PROTO}/results/proto.entries.json"))


def fee(row, c):
    """A measured Testnet fee, with its rent rescaled to rate c."""
    return (row["fee"] - row["rent"]) + row["rent"] * c["rent_fee_1kb"] / T["rent_fee_1kb"]


def cur(op, c):
    return fee(CUR[op], c)


def pro(prefix, c):
    return fee(next(r for p, r in PRO.items() if p.startswith(prefix)), c)


# --- assumptions (stated in the doc) -----------------------------------------
LIFE = 270            # days a deposit is in store
PLEDGED = 0.30        # share of deposits pledged
PLEDGE_AT, LOAN = 30, 90   # pledged on day 30 for 90 days
PER_WH_MONTH = 50     # deposits per warehouse per month
BATCH = CUR["extend_ttl_batch — 25 receipts, TTL above threshold"]["fee"] / 25


def keepalive(c, size, days, entries=1):
    """Renewal rent + overhead for an entry living `days` after being
    written (the write prepays 175 days)."""
    bought, n, _ = strat(30, 175, days)
    return rent(c, size, bought * DAY) + n * (entries * TTLW + BATCH)


DAYROOT_TTL = 30     # days a day root stays on-chain (temporary storage)


def temp_rent(c, size, days):
    return -(-size * c["rent_fee_1kb"] * days * DAY // (1024 * 2430))


def anchor_fee(c):
    """One daily anchor as recommended: the head (persistent, measured) plus
    the day's root in temporary storage for DAYROOT_TTL days, so proofs can
    be checked on-chain (computed: temporary rent is half persistent; the
    rent formula matched charged fees within 0.1%)."""
    day_root = PRO_E["B day root"]
    return pro("B b_anchor — head only", c) + temp_rent(c, day_root, DAYROOT_TTL) \
        + TTLW + day_root * 875 // 1024


def head_rent_per_year(c):
    return rent(c, PRO_E["B head"], 365 * DAY) + 2 * (TTLW + BATCH)


def anchors_per_warehouse_year(c):
    return 365 * anchor_fee(c) + head_rent_per_year(c)


def anchor_per_deposit(c, monthly):
    """Anchors are per warehouse per day, shared by its deposits."""
    warehouses = max(1, monthly // PER_WH_MONTH)
    return warehouses * anchors_per_warehouse_year(c) / 12 / monthly


R_CUR = CUR_E["Receipt — Active, freshly minted"]["bytes"]
G_CUR = CUR_E["DepositIndex"]["bytes"]
R_A = PRO_E["A receipt"]


def current(c, monthly):
    base = cur("mint — farmer's later receipt", c) + cur("exit — full", c) \
        + keepalive(c, R_CUR + G_CUR, LIFE, 2)
    pledge = cur("lock — full", c) + cur("unlock — full", c)
    return base + PLEDGED * pledge


def design_a(c, monthly):
    # Only pledged deposits touch the chain: minted at pledge time, then the
    # current lifecycle. The bitmap chunk (one per 1,024 deposits) is
    # amortised in the "later deposit" mint; its creation is spread below.
    on_chain = pro("A a_mint — later", c) + cur("lock — full", c) + cur("unlock — full", c) \
        + cur("exit — full", c) + keepalive(c, R_A, LIFE - PLEDGE_AT)
    chunk = (pro("A a_mint — first", c) - pro("A a_mint — later", c)) / 1024
    return PLEDGED * on_chain + chunk


def design_ab(c, monthly):
    return design_a(c, monthly) + anchor_per_deposit(c, monthly)


def agg_lot(c, lots):
    """C: the average cost of adding one of `lots` lots, plus that lot's
    share of the aggregate's keep-alive."""
    adds = [pro(f"C c_add_lot — lot {n} ", c) for n in range(1, lots + 1)]
    # lot 1 also created a bitmap chunk; that is amortised separately
    adds[0] -= pro("A a_mint — first", c) - pro("A a_mint — later", c)
    size = PRO_E[f"C aggregate entry, {lots} lot(s)"]
    keep = keepalive(c, size, LIFE - PLEDGE_AT) / lots
    return sum(adds) / lots + keep


def design_abc(c, monthly, lots):
    pledge = pro("C c_pledge_lot", c)
    # unlock and exit rewrite the aggregate the same way a pledge does
    on_chain = agg_lot(c, lots) + 3 * pledge
    chunk = (pro("A a_mint — first", c) - pro("A a_mint — later", c)) / 1024
    return PLEDGED * on_chain + chunk + anchor_per_deposit(c, monthly)


def design_d(c, monthly):
    on_chain = pro("D d_pledge — proof depth 11, later", c) + pro("D d_release", c)
    chunk = (pro("D d_pledge — proof depth 11, first", c) - pro("D d_pledge — proof depth 11, later", c)) / 1024
    return PLEDGED * on_chain + chunk + anchor_per_deposit(c, monthly)


DESIGNS = [
    ("Current (`data_version` 2)", current),
    ("A: mint on demand", design_a),
    ("A+B: + daily anchoring", design_ab),
    ("A+B+C: + aggregated lots (3 per entry)", lambda c, m: design_abc(c, m, 3)),
    ("D: minimal chain", design_d),
]

if __name__ == "__main__":
    for name, fn in DESIGNS:
        print(name)
        for rname, c in RATES:
            v = [fn(c, mth) / X for mth in (200, 2000)]
            print(f"   {rname}: per deposit {v[0]:.4f} / {v[1]:.4f}  monthly {200*v[0]:,.1f} / {2000*v[1]:,.1f}")
