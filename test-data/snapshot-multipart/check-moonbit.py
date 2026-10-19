#!/usr/bin/env python3
"""Run the shared framing corpus against the MoonBit SDK's private decoder."""

import json
from pathlib import Path
import subprocess


def byte_literal(value: bytes) -> str:
    return 'b"' + ''.join(f'\\x{byte:02x}' for byte in value) + '"'


root = Path(__file__).resolve().parents[2]
corpus = json.loads(Path(__file__).with_name("framing.json").read_text())
sdk = root / "sdks/moonbit/golem_sdk"
fixture = sdk / "agents/framing_corpus_wbtest.mbt"
cases = []
for case in corpus["valid"]:
    newline = case["newline"]
    boundary = corpus["boundary"]
    # Input is built independently; the expected payload comes from the corpus hex.
    wire = (
        f'--{boundary}{newline}Content-Disposition: attachment; name="state"{newline}'
        f'Content-Type: application/json{newline}{newline}'
        '{"version":1,"principal":{"tag":"anonymous"},"state":null}'
        f'{newline}--{boundary}{newline}Content-Disposition: attachment; name="part:index"{newline}'
        f'Content-Type: application/octet-stream{newline}{newline}'
        f'{case["payload"]}{newline}--{boundary}--{newline}'
    ).encode("utf-8")
    cases.append(f'''///|
test "shared corpus: {case['name']}" {{
  let wire = {byte_literal(wire)}
  guard decode_multipart_snapshot(wire, "multipart/mixed; boundary={boundary}")
    is Ok((_, saved)) else {{ fail("valid shared wire rejected") }}
  assert_eq(saved.require_part("index", "application/octet-stream"), Ok({byte_literal(bytes.fromhex(case['hex']))}))
}}
''')

# Do not replace a developer's existing file, even if a prior run was interrupted.
with fixture.open("x") as output:
    output.write("\n".join(cases))
try:
    subprocess.run(["./scripts/run-sdk-tests.sh", "agents/framing_corpus_wbtest.mbt"], cwd=sdk, check=True)
finally:
    fixture.unlink()
