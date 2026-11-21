#!/usr/bin/env python3
"""Write shards/index.json: {"<crate>": {"<version>": "<shard>"}} for every registry package in
every shard's Cargo.lock, so the flake can find the shard that carries a crate without reading
1,630 lock files. Run after gen-crates (the relock workflow does)."""
import json, os, re, sys

root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "shards")
pkg = re.compile(r'^\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\nsource = "registry\+', re.M)
index = {}
shards = sorted(d for d in os.listdir(root) if d.isdigit())
for n in shards:
    try:
        with open(os.path.join(root, n, "Cargo.lock"), encoding="utf-8") as f:
            lock = f.read()
    except FileNotFoundError:
        continue
    for name, version in pkg.findall(lock):
        index.setdefault(name, {})[version] = n
with open(os.path.join(root, "index.json"), "w", encoding="utf-8") as f:
    json.dump(index, f, separators=(",", ":"), sort_keys=True)
print(f"{sum(len(v) for v in index.values())} crate versions in {len(shards)} shards -> shards/index.json", file=sys.stderr)
