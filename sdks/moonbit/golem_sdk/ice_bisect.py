#!/usr/bin/env python3
import subprocess, sys, os

SRC = "gen/interface/golem/agent/guest/ffi.mbt"
TMP = "/tmp/ice_subset.mbt"

with open(SRC) as f:
    text = f.read()

# Split into top-level blocks separated by lines that are exactly "///|"
lines = text.splitlines(keepends=True)
blocks = []
cur = []
for ln in lines:
    if ln.strip() == "///|":
        if cur:
            blocks.append("".join(cur))
        cur = [ln]
    else:
        cur.append(ln)
if cur:
    blocks.append("".join(cur))

def ice(idxs):
    """Return True if the ICE (exit 199) fires for the given block indices."""
    with open(TMP, "w") as f:
        f.write("".join(blocks[i] for i in idxs))
    r = subprocess.run(["bash", "ice_check.sh", TMP], capture_output=True, text=True)
    out = r.stdout + r.stderr
    if "output_value: integer cannot be read back" in out:
        return True
    return False

if __name__ == "__main__":
    n = len(blocks)
    print(f"total blocks: {n}", file=sys.stderr)
    cmd = sys.argv[1] if len(sys.argv) > 1 else "bisect"
    if cmd == "count":
        sys.exit(0)
    if cmd == "test":
        # test explicit comma-separated indices
        idxs = [int(x) for x in sys.argv[2].split(",")]
        print("ICE" if ice(idxs) else "no-ICE")
        sys.exit(0)
    # binary search: assume a single triggering block among all; keep candidate set
    candidates = list(range(n))
    # sanity: full set must ICE
    if not ice(candidates):
        print("FULL SET DOES NOT ICE - assumption broken", file=sys.stderr)
        sys.exit(1)
    while len(candidates) > 1:
        mid = len(candidates) // 2
        first = candidates[:mid]
        if ice(first):
            candidates = first
            where = "first"
        else:
            candidates = candidates[mid:]
            where = "second"
        print(f"narrowed to {len(candidates)} blocks ({where}: {candidates[0]}..{candidates[-1]})", file=sys.stderr)
    b = candidates[0]
    print(f"TRIGGER BLOCK INDEX: {b}", file=sys.stderr)
    print("=== BLOCK CONTENT ===")
    print(blocks[b])
