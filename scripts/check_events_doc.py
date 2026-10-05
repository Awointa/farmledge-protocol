#!/usr/bin/env python3
"""Fails if docs/events.md and contracts/receipt/src/events.rs disagree.

    scripts/check_events_doc.py              # against the source
    scripts/check_events_doc.py --deployed   # and the deployed Wasm's spec

For every `#[contractevent]` struct: the event is documented under its
snake_case name, with the same fields in the same order, the same types, and
the same topic fields (marked T, in order). Also checks that every event is
published somewhere in the contract, and named in the state-transition
table. The backend is written against the document, so it must match the
code exactly.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "contracts/receipt/src"


def snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def code_events():
    text = (SRC / "events.rs").read_text()
    out = {}
    for m in re.finditer(r"#\[contractevent\]\s*pub struct (\w+) \{(.*?)\n\}", text, re.S):
        fields, topic = [], False
        for line in m.group(2).splitlines():
            line = line.strip()
            if line == "#[topic]":
                topic = True
            fm = re.match(r"pub (\w+): (.+),$", line)
            if fm:
                fields.append((fm.group(1), fm.group(2), topic))
                topic = False
        out[snake(m.group(1))] = (m.group(1), fields)
    return out


def doc_events(doc):
    out = {}
    # Per-event tables: "### `name`: ..." then | field | T | type | notes |
    for m in re.finditer(r"^### `(\w+)`.*?\n\n(\|.*?)(?:\n\n|\Z)", doc, re.S | re.M):
        rows = [r for r in m.group(2).splitlines()[2:] if r.startswith("|")]
        fields = []
        for r in rows:
            cells = [c.strip() for c in r.strip("|").split("|")]
            fields.append((cells[0], cells[2], cells[1] == "T"))
        out[m.group(1)] = fields
    # The registry table: | `name` | topic: Type | a: Type, b: Type |
    for m in re.finditer(r"^\| `(\w+)` \| (.+?) \| (.+?) \|$", doc, re.M):
        name, topics, data = m.groups()
        if name in out or ":" not in topics:
            continue
        parse = lambda s, t: [(f.split(":")[0].strip(), f.split(":")[1].strip(), t)
                              for f in s.split(",")]
        out[name] = parse(topics, True) + parse(data, False)
    return out


def spec_type(t):
    """A contract-spec type as it is written in Rust."""
    if isinstance(t, str):
        return {"u64": "u64", "u32": "u32", "bool": "bool", "address": "Address",
                "symbol": "Symbol", "string": "String"}[t]
    if "bytes_n" in t:
        return f"BytesN<{t['bytes_n']['n']}>"
    if "option" in t:
        return f"Option<{spec_type(t['option']['value_type'])}>"
    if "udt" in t:
        return t["udt"]["name"]
    raise ValueError(t)


def deployed_events():
    """Events in the spec of the Wasm deployed per deployments/testnet.json."""
    record = json.loads((ROOT / "deployments/testnet.json").read_text())
    res = subprocess.run(["stellar", "contract", "info", "interface", "--network",
                          record["network"], "--contract-id", record["contract_id"],
                          "--output", "json"], capture_output=True, text=True, check=True)
    out = {}
    for entry in json.loads(res.stdout):
        if "event_v0" not in entry:
            continue
        e = entry["event_v0"]
        # stellar-cli 27 names the key "type_", 28 "type".
        out[e["prefix_topics"][0]] = [(p["name"], spec_type(p.get("type", p.get("type_"))),
                                       p["location"] == "topic_list") for p in e["params"]]
    return record["contract_id"], out


def main():
    doc = (ROOT / "docs/events.md").read_text()
    code, documented = code_events(), doc_events(doc)
    rust = "".join(p.read_text() for p in SRC.glob("*.rs") if p.name != "events.rs")
    helpers = (SRC / "events.rs").read_text().split("// --- emit helpers")[1]
    transitions = doc.split("## State transitions and their events")[1].split("\n## ")[0]
    errors = []
    for name, (struct, fields) in code.items():
        if name not in documented:
            errors.append(f"{name}: not documented")
            continue
        if documented[name] != fields:
            errors.append(f"{name}: documented {documented[name]}\n    code       {fields}")
        topics = [f for f, _, t in fields if t]
        # Topics must lead, in declaration order, as the encoding section says.
        if [f for f, _, t in fields[:len(topics)]] != topics:
            errors.append(f"{name}: topic fields are not first")
        if f"events::{struct} {{" not in rust and f"{struct} {{" not in helpers:
            errors.append(f"{name}: never published")
        if f"`{name}`" not in transitions:
            errors.append(f"{name}: missing from the state-transition table")
    for name in documented.keys() - code.keys():
        errors.append(f"{name}: documented but not in events.rs")
    if "--deployed" in sys.argv:
        contract, deployed = deployed_events()
        for name in deployed.keys() | documented.keys():
            if deployed.get(name) != documented.get(name):
                errors.append(f"{name}: deployed {contract[:8]}… has {deployed.get(name)}\n"
                              f"    documented {documented.get(name)}")
        print(f"compared with the spec of deployed contract {contract}")
    if errors:
        print("docs/events.md does not match events.rs:\n  " + "\n  ".join(errors))
        sys.exit(1)
    print(f"docs/events.md matches events.rs{' and the deployed spec' if '--deployed' in sys.argv else ''}: {len(code)} events, "
          f"{sum(len(f) for _, f in code.values())} fields")


if __name__ == "__main__":
    main()
