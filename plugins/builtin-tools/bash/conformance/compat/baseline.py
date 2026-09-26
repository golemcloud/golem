"""Compare a compat-suite run with the recorded baseline of cases known to fail.

    baseline.py check OUTPUT    fail on a case that fails but is not in the baseline, and on one
                                in the baseline that now passes (so fixed cases leave it)
    baseline.py update OUTPUT   rewrite the baseline from OUTPUT

OUTPUT is what `run.sh` (or `run-in-docker.sh`) printed. The baseline, `baseline.txt` beside this
script, holds one failing case name per line, sorted. Brush's own known failures (`known_failure`
in its YAML) are the harness's business and never appear in either list.
"""
import pathlib
import re
import sys

BASELINE = pathlib.Path(__file__).with_name("baseline.txt")


def failing(output):
    """The cases a run reported as FAILED, each with the harness's report of how it differed."""
    text = re.sub(r"\x1b\[[0-9;]*m", "", pathlib.Path(output).read_text(errors="replace"))
    reports = {}
    for block in re.split(r"\n(?=\* Test case: \[)", text):
        match = re.match(r"\* Test case: \[(.*?)\]\.\.\.", block)
        if match and "\n    FAILED." in block:
            reports[match.group(1)] = block.split("\n    FAILED.")[0].rstrip()
    if "test case(s) ran" not in text:
        sys.exit(f"{output}: no summary line; the run did not finish")
    return reports


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("check", "update"):
        sys.exit(__doc__)
    command, output = sys.argv[1:]
    now = failing(output)
    if command == "update":
        BASELINE.write_text("".join(f"{name}\n" for name in sorted(now)))
        print(f"{len(now)} failing cases recorded in {BASELINE.name}")
        return
    known = set(BASELINE.read_text().splitlines())
    new, fixed = sorted(set(now) - known), sorted(known - set(now))
    for name in new:
        # The harness's own report, so a failure seen only in CI can be read from its log.
        print(f"NEW FAILURE {name}\n{now[name]}")
    for name in fixed:
        print(f"NOW PASSES {name} (remove it from {BASELINE.name})")
    print(f"{len(now)} failing, {len(known)} in the baseline; {len(new)} new, {len(fixed)} fixed")
    sys.exit(1 if new or fixed else 0)


if __name__ == "__main__":
    main()
