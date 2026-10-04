# Storage-strategy prototype (THROWAWAY)

**This is not Farmledge code and is not deployed as Farmledge.** It exists
only so the figures in [`docs/storage-strategy.md`](../../docs/storage-strategy.md)
can be reproduced. Nothing here is built by `make`, CI or the workspace, and
nothing in production may depend on it. Do not copy it into the contract:
A+B will be implemented properly, from its own design and audit.

A Testnet instance was deployed once, to measure real charged fees:
`CBSD3ZLE6Y6IKNKYGHG663DWZ6ZESST4V3QHICVTKRWHUCB2ZKRSB6S5`. Its on-chain
metadata says "THROWAWAY PROTOTYPE … Not a Farmledge deployment. Do not use."
It is deliberately absent from `deployments/`. Nothing should call it.

## What it is

The receipt contract at commit `0f5a3a3`, plus one module, `src/proto.rs`,
which adds:

| function | design | what it does |
|---|---|---|
| `a_mint` | A | `mint` with a per-warehouse deposit-number bitmap instead of the `DepositIndex` guard |
| `b_anchor`, `b_verify`, `leaf_hash` | B | hash-chained daily head, optional day root, Merkle proof check, the leaf hash |
| `c_add_lot`, `c_pledge_lot` | C | one entry per (farmer, warehouse, commodity) |
| `d_pledge`, `d_release` | D | a pledge created from an anchored leaf |

**Known flaw, kept as found:** `d_pledge` guards against double pledges by
leaf (warehouse, day, index). A lot gets a new balance leaf every day, so the
same lot can be pledged twice with two days' leaves. See
`docs/storage-strategy.md`. Its other shortcuts (no recovery, no exits, no
indexes for C) are what a cost measurement needed, not designs.

## Files

| path | |
|---|---|
| `src/proto.rs`, `src/proto_probe.rs` | the prototype module and its test-host test |
| `build.sh` | overlays them on the contract at `0f5a3a3` in a temp dir, runs the test, builds `out/farmledge_storage_proto.wasm` (`build.sh deploy` also deploys to Testnet) |
| `measure_proto.py` | the Testnet scenario; reads `results/prototype-contract.json`, writes `results/proto.*.json` |
| `model/strategy_model.py` | the cost model: per-deposit and monthly figures for every design and rate |
| `data/*.settings.2026-10-04.json` | Testnet and mainnet config snapshots the rates come from |
| `results/` | the measured fees and entry sizes behind the doc |

## Reproducing

```bash
prototypes/storage/build.sh                       # Wasm hash 14a13504…0a18, same as the deployed prototype
python3 prototypes/storage/model/strategy_model.py   # the cost tables, from the committed results
```

Remeasuring needs a fresh prototype deployment (`build.sh deploy`), its id
written to `results/prototype-contract.json`, and
`python3 prototypes/storage/measure_proto.py`. It spends Testnet XLM only.
