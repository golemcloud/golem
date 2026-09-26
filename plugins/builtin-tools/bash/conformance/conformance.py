#!/usr/bin/env python3
"""Compare the bash tool's shell with Bash 5 and GNU tools, case by case.

Both sides run in the environment a Golem agent has: no HOME, a uid with no passwd entry, cwd `/`,
Golem's GOLEM_* variables, and a fresh `/tmp`. The oracle's answers are recorded once into
`goldens/` so that checking needs no Docker.

    conformance.py check --wasm W     compare our shell with the recorded goldens (the default);
                                      `--tier all` adds the sweep-tier corpora the slow job checks
    conformance.py record             run the oracle twice per case (`--runs 1` for one) and
                                      rewrite goldens for the selected cases; refuses a case its
                                      runs disagree on
    conformance.py live --wasm W      compare our shell with the oracle directly
    conformance.py stale              re-run the oracle and report goldens that no longer match
    conformance.py coverage --wasm W  fail if a feature in features.py has no case
"""
import argparse
import base64
import concurrent.futures
import hashlib
import importlib.util
import json
import os
import pathlib
import queue
import re
import subprocess
import sys
import tempfile
import threading
import uuid

HERE = pathlib.Path(__file__).resolve().parent
CORPORA = HERE / "cases"
GOLDENS = HERE / "goldens"
DOCKERFILE = HERE / "oracle.Dockerfile"
ORACLE_IMAGE = "golem-cooperative-bash5-oracle:local"
TIMEOUT = 15  # Default per-case run timeout, either side; see `Case.timeout` to override one.
# A line holding this comment, optionally followed by a directory, splits a script into separate
# calls. Each call is a fresh shell, as each `run` of the tool is: our shell's example starts every
# call in the directory the previous one ended in, as a caller passing back the returned `cwd`
# does. The oracle runs each call as its own `bash -c` in the directory the marker names (`/` when
# it names none), so a case states where bash ends up rather than having the harness track it.
CALL_MARKER = "#--call--"
# A corpus module sets `TIER = "sweep"` to be checked only by the slow job (`check --tier all`);
# every other module is checked on every PR.
TIERS = ("pr", "sweep")

# What a Golem agent's shell sees, on both sides. The oracle also gets a PATH and a locale, since
# GNU bash must find the tools and our shell always uses UTF-8; cases that print them are fixtures.
GOLEM_ENV = {
    "GOLEM_AGENT_ID": "conformance-agent",
    "GOLEM_WORKER_NAME": "conformance-agent",
    "GOLEM_COMPONENT_ID": "00000000-0000-0000-0000-000000000000",
    "GOLEM_COMPONENT_REVISION": "0",
    "GOLEM_AGENT_TYPE": "Conformance",
}
ORACLE_ENV = {"LC_ALL": "C.UTF-8", "PATH": "/usr/local/bin:/usr/bin:/bin"}
ORACLE_USER = "1000:1000"
# Bump when the oracle's setup changes in a way that changes answers; it invalidates goldens.
ORACLE_SETUP = "writable-root-v2"


class Case:
    """One script, the corpus it came from, its feature tags and any deliberate expectation."""

    def __init__(self, name, script, corpus, tags=(), options=None, tier="pr"):
        self.name = name
        self.script = script
        self.corpus = corpus
        self.tags = tuple(tags)
        self.options = dict(options or {})
        self.tier = tier  # "pr", or "sweep" for corpora only the slow job checks
        self.deliberate = None  # (status, stdout, stderr, reason)
        self.deliberate_stderr = None  # (stderr, reason)

    @property
    def script_hash(self):
        return hashlib.sha256(self.script.encode()).hexdigest()[:16]

    @property
    def timeout(self):
        """How long a run of this case (either side) may take, in seconds. Cases that need
        longer than the default -- an endless-recursion crash under a loaded CI runner, say --
        set it via `OPTIONS = {"case name": {"timeout": 60}}` in their corpus module, the same
        way `no_tmp` is set."""
        return self.options.get("timeout", TIMEOUT)


def _expected_entry(value, default_reason, where):
    if len(value) == 4:
        return tuple(value)
    if len(value) == 3 and default_reason:
        return (*value, default_reason)
    raise ValueError(f"{where}: a deliberate expectation needs a reason")


def _stderr_entry(value, default_reason, where):
    if isinstance(value, tuple) and len(value) == 2:
        return value
    if isinstance(value, bytes) and default_reason:
        return (value, default_reason)
    raise ValueError(f"{where}: a deliberate stderr needs a reason")


def load_cases(corpora=CORPORA):
    """Load every corpus module in cases/, validating names, fixtures and their reasons."""
    cases = []
    by_name = {}
    for path in sorted(corpora.glob("*.py")):
        spec = importlib.util.spec_from_file_location(f"cases_{path.stem}", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        default_tags = tuple(getattr(module, "TAGS", ()))
        if getattr(module, "ERROR_PATHS", False):
            default_tags += ("error",)
        options = getattr(module, "OPTIONS", {})
        tier = getattr(module, "TIER", "pr")
        if tier not in TIERS:
            raise ValueError(f"{path.name}: TIER is {tier!r}, not one of {TIERS}")
        local = {}
        for entry in module.CASES:
            name, script = entry[0], entry[1]
            tags = default_tags + tuple(entry[2] if len(entry) > 2 else ())
            if name in by_name:
                raise ValueError(f"duplicate case name {name!r} in {path.name}")
            case = Case(name, script, path.stem, tags, options.get(name), tier)
            by_name[name] = local[name] = case
            cases.append(case)
        reason = getattr(module, "EXPECTED_REASON", None)
        for name, value in getattr(module, "EXPECTED", {}).items():
            where = f"{path.name}: EXPECTED[{name!r}]"
            if name not in local:
                raise ValueError(f"{where} names no case in this corpus")
            local[name].deliberate = _expected_entry(value, reason, where)
        reason = getattr(module, "EXPECTED_STDERR_REASON", None)
        for name, value in getattr(module, "EXPECTED_STDERR", {}).items():
            where = f"{path.name}: EXPECTED_STDERR[{name!r}]"
            if name not in local:
                raise ValueError(f"{where} names no case in this corpus")
            local[name].deliberate_stderr = _stderr_entry(value, reason, where)
        unknown = set(options) - set(local)
        if unknown:
            raise ValueError(f"{path.name}: OPTIONS names no case: {sorted(unknown)}")
    return cases


# --- Outcomes ----------------------------------------------------------------------------------


def outcome(result):
    """Return the exact process result asserted by every case."""
    return result.returncode, result.stdout, result.stderr


def wasm_outcome(result, status_file):
    """Read the shell status separately from WASI's success/failure process status."""
    try:
        status = int(status_file.read_text(encoding="ascii"))
    except (OSError, ValueError) as error:
        raise ValueError(
            f"missing or invalid shell status report: {error}; stderr={result.stderr!r}"
        ) from error
    if not 0 <= status <= 255:
        raise ValueError(f"shell status is outside the u8 range: {status}")
    process_status = 0 if status == 0 else 1
    if result.returncode != process_status:
        raise ValueError(
            f"WASI process status {result.returncode} disagrees with reported shell status {status}"
        )
    return status, result.stdout, result.stderr


def split_calls(script):
    """The calls a script makes, as (starting directory, script) pairs; one pair without markers."""
    calls, cwd, current = [], "/", []
    for line in script.split("\n"):
        if line == CALL_MARKER or line.startswith(CALL_MARKER + " "):
            calls.append((cwd, "\n".join(current)))
            cwd, current = line[len(CALL_MARKER):].strip() or "/", []
        else:
            current.append(line)
    calls.append((cwd, "\n".join(current)))
    return calls


def outcomes_match(expected, actual):
    """Keep this tiny comparison separately testable: no stderr filtering is permitted."""
    return actual == expected


def encode_bytes(data):
    try:
        return {"text": data.decode("utf-8")}
    except UnicodeDecodeError:
        return {"base64": base64.b64encode(data).decode("ascii")}


def decode_bytes(value):
    if "text" in value:
        return value["text"].encode("utf-8")
    return base64.b64decode(value["base64"])


def encode_outcome(case, result):
    status, stdout, stderr = result
    return {
        "script": case.script_hash,
        "status": status,
        "stdout": encode_bytes(stdout),
        "stderr": encode_bytes(stderr),
    }


def decode_outcome(entry):
    return entry["status"], decode_bytes(entry["stdout"]), decode_bytes(entry["stderr"])


def expected_for(case, oracle):
    """The expectation for a case: a deliberate fixture, or the oracle with any stderr override."""
    if case.deliberate is not None:
        return case.deliberate[:3]
    if case.deliberate_stderr is not None:
        return (oracle[0], oracle[1], case.deliberate_stderr[0])
    return oracle


# --- Goldens ----------------------------------------------------------------------------------


def oracle_fingerprint():
    """What the goldens depend on: the oracle image definition and the environment profile."""
    profile = json.dumps(
        {"env": GOLEM_ENV, "oracle_env": ORACLE_ENV, "user": ORACLE_USER, "setup": ORACLE_SETUP},
        sort_keys=True,
    )
    return {
        "dockerfile": hashlib.sha256(DOCKERFILE.read_bytes()).hexdigest()[:16],
        "profile": hashlib.sha256(profile.encode()).hexdigest()[:16],
    }


def golden_path(corpus, goldens=GOLDENS):
    return goldens / f"{corpus}.json"


def read_goldens(corpus, goldens=GOLDENS):
    path = golden_path(corpus, goldens)
    if not path.exists():
        return {"oracle": None, "cases": {}}
    return json.loads(path.read_text(encoding="utf-8"))


def write_goldens(corpus, data, goldens=GOLDENS):
    goldens.mkdir(parents=True, exist_ok=True)
    text = json.dumps(data, indent=1, sort_keys=True, ensure_ascii=False) + "\n"
    golden_path(corpus, goldens).write_text(text, encoding="utf-8")


def golden_for(case, loaded, fingerprint):
    """The recorded oracle outcome for a case, or an explanation of why it is unusable."""
    data = loaded.get(case.corpus)
    if data is None or data.get("oracle") != fingerprint:
        return None, f"goldens for {case.corpus} were recorded with another oracle; re-record"
    entry = data["cases"].get(case.name)
    if entry is None:
        return None, "no golden; run `conformance.py record` for this case"
    if entry["script"] != case.script_hash:
        return None, "golden is for an older script; re-record"
    return decode_outcome(entry), None


# --- Running our shell ----------------------------------------------------------------------


def run_ours(case, wasm, wasmtime, base, cache):
    """Run one case in the wasm shell with a fresh root; return its outcome."""
    root = pathlib.Path(tempfile.mkdtemp(dir=base))
    if not case.options.get("no_tmp"):
        (root / "tmp").mkdir()
    report = pathlib.Path(tempfile.mkdtemp(dir=base)) / "status"
    command = [
        wasmtime, "run", "-Sp3", "-Shttp", "-Wcomponent-model-async=y",
        "-Ccache-config=" + str(cache), "--dir", str(root) + "::/",
        "--dir", str(report.parent) + "::/.conformance",
    ]
    for key, value in GOLEM_ENV.items():
        command += ["--env", f"{key}={value}"]
    command += [str(wasm), "--status-file", "/.conformance/status"]
    calls = split_calls(case.script)
    if len(calls) > 1:
        # The example carries each call's final directory itself; the markers' are the oracle's.
        command += ["--calls", *(script for _, script in calls)]
    else:
        command.append(case.script)
    result = subprocess.run(command, capture_output=True, timeout=case.timeout)
    return wasm_outcome(result, report)


# --- The oracle ---------------------------------------------------------------------------------


class Oracle:
    """A long-lived oracle container; cases run one at a time with `docker exec`.

    As in a Golem agent, the case's user may write anywhere in the filesystem: `/` is made
    world-writable. Before each case, every process the previous one left behind is killed, every
    top-level entry it created is removed, and `/tmp` is recreated empty (or left out, for a
    `no_tmp` case), so cases stay independent without paying for a container per case.
    """

    def __init__(self, image):
        self.name = f"golem-conformance-{uuid.uuid4().hex[:12]}"
        subprocess.run(
            [
                "docker", "run", "-d", "--rm", "--name", self.name, "--user", ORACLE_USER,
                "--network", "none", "--workdir", "/", "--entrypoint", "sleep", image, "infinity",
            ],
            check=True, capture_output=True, timeout=120,
        )
        baseline = self.root("chmod 1777 / && ls -A /").stdout.decode().split()
        self.baseline = " ".join(entry for entry in baseline if entry != "tmp")

    def root(self, script):
        return subprocess.run(
            ["docker", "exec", "-u", "0", self.name, "sh", "-c", script],
            check=True, capture_output=True, timeout=60,
        )

    def reset(self, with_tmp):
        self.root(
            "kill -9 -1 2>/dev/null; cd / || exit 1; "
            f"for entry in $(ls -A /); do case \" {self.baseline} \" in *\" $entry \"*) ;; "
            '*) rm -rf "/$entry" ;; esac; done; '
            + ("mkdir -m 1777 /tmp" if with_tmp else "true")
        )

    def run(self, case):
        """Run each of the case's calls as its own `bash -c`; output accumulates, and the status
        is the last call's, as the example reports them."""
        self.reset(with_tmp=not case.options.get("no_tmp"))
        env = [f"{key}={value}" for key, value in {**ORACLE_ENV, **GOLEM_ENV}.items()]
        status, stdout, stderr = 0, b"", b""
        for cwd, script in split_calls(case.script):
            command = [
                "docker", "exec", "-u", ORACLE_USER, "-w", cwd, self.name,
                "env", "-i", *env, "bash", "-c", script,
            ]
            status, out, err = outcome(
                subprocess.run(command, capture_output=True, timeout=case.timeout)
            )
            stdout, stderr = stdout + out, stderr + err
        return status, stdout, stderr

    def close(self):
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True, timeout=60)


def run_oracle(cases, image, jobs, report):
    """Run cases on `jobs` oracle containers; return {name: outcome or error string}."""
    work = queue.Queue()
    for case in cases:
        work.put(case)
    results = {}
    lock = threading.Lock()

    def worker():
        oracle = Oracle(image)
        try:
            while True:
                try:
                    case = work.get_nowait()
                except queue.Empty:
                    return
                try:
                    result = oracle.run(case)
                except subprocess.TimeoutExpired:
                    result = f"oracle exceeded {case.timeout} seconds"
                with lock:
                    results[case.name] = result
                    report(case, result)
        finally:
            oracle.close()

    threads = [threading.Thread(target=worker) for _ in range(max(1, min(jobs, len(cases))))]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return results


# --- Commands ---------------------------------------------------------------------------------


def select(cases, args):
    if not args.case and not args.prefix:
        return cases
    return [
        case for case in cases
        if case.name in args.case or any(case.name.startswith(p) for p in args.prefix)
    ]


def build_oracle(image):
    subprocess.run(
        ["docker", "build", "-q", "-f", str(DOCKERFILE), "-t", image, str(HERE)],
        check=True, capture_output=True,
    )


def differing_field(a, b):
    """The first of status/stdout/stderr where two raw oracle outcomes disagree, or None."""
    for field, left, right in zip(("status", "stdout", "stderr"), a, b):
        if left != right:
            return field
    return None


def command_record(args, cases):
    """Run the oracle `args.runs` times per case (a fresh container each time -- each call below
    is an independent `run_oracle`, which starts new containers per worker) and only record a
    case whose runs all agree. A case whose answer depends on this run's own randomness (a PID,
    `$RANDOM`, wall-clock time not pinned by the case itself, ...) would otherwise poison the
    golden with an answer that stops matching the very next time the oracle happens to run it --
    exactly what `stale` exists to catch, except recorded once, silently, and only noticed later.
    Catching it here instead, at the point a case is added or its script changes, means `stale`
    only ever has to explain a REAL change in the oracle, not the corpus's own flakiness.
    """
    build_oracle(args.oracle_image)
    fingerprint = oracle_fingerprint()
    errored = set()

    def report(case, result):
        if isinstance(result, str):
            errored.add(case.name)
            print(f"ERROR {case.name}: {result}", flush=True)

    runs = [run_oracle(cases, args.oracle_image, args.jobs, report) for _ in range(max(1, args.runs))]

    nondeterministic = set()
    final = {}
    for case in cases:
        if case.name in errored:
            continue
        values = [run.get(case.name) for run in runs]
        if any(value is None for value in values):
            # A worker thread that never reported at all (its container failed to start, say) --
            # rare, and not the same `errored` a caught TimeoutExpired already covers, but the
            # golden must not be written from a partial set of runs either.
            errored.add(case.name)
            print(f"ERROR {case.name}: an oracle run reported no result", flush=True)
            continue
        first = values[0]
        mismatch = next((differing_field(first, other) for other in values[1:] if differing_field(first, other)), None)
        if mismatch:
            nondeterministic.add(case.name)
            print(f"NONDETERMINISTIC {case.name}: {mismatch} differs between runs", flush=True)
            continue
        final[case.name] = first

    by_corpus = {}
    for case in cases:
        by_corpus.setdefault(case.corpus, []).append(case)
    all_names = {case.name for case in load_cases()}
    for corpus, members in by_corpus.items():
        data = read_goldens(corpus)
        if data.get("oracle") != fingerprint:
            data = {"oracle": fingerprint, "cases": {}}
        for case in members:
            result = final.get(case.name)
            if isinstance(result, tuple):
                data["cases"][case.name] = encode_outcome(case, result)
        data["cases"] = {k: v for k, v in data["cases"].items() if k in all_names}
        write_goldens(corpus, data)
    print(f"recorded {len(final)} of {len(cases)} cases", flush=True)
    if nondeterministic:
        print(
            f"{len(nondeterministic)} non-deterministic, not recorded: {', '.join(sorted(nondeterministic))}",
            flush=True,
        )
    return not errored and not nondeterministic


def failure_record(case, expected, actual, error):
    """One row of the triage report: enough to judge the difference without re-running it."""
    def side(result):
        if result is None:
            return None
        status, stdout, stderr = result
        return {"status": status, "stdout": encode_bytes(stdout), "stderr": encode_bytes(stderr)}

    return {
        "name": case.name, "corpus": case.corpus, "tags": list(case.tags), "script": case.script,
        "expected": side(expected), "actual": side(actual), "error": error,
    }


def compare_ours(args, cases, expectation):
    """Run our shell for each case and compare with `expectation(case)` -> (expected, error)."""
    wasm = args.wasm.resolve()
    failures = []
    records = []
    with tempfile.TemporaryDirectory(prefix="golem-conformance-") as directory:
        base = pathlib.Path(directory)
        cache = base / "cache.toml"
        cache.write_text('[cache]\ndirectory = "' + str(base / "cache") + '"\n')

        def one(case):
            expected, error = expectation(case)
            if error:
                records.append(failure_record(case, None, None, error))
                return f"FAIL {case.name}: {error}"
            try:
                actual = run_ours(case, wasm, args.wasmtime, base, cache)
            except subprocess.TimeoutExpired:
                records.append(
                    failure_record(case, expected, None, f"exceeded {case.timeout} seconds")
                )
                return f"FAIL {case.name} exceeded {case.timeout} seconds"
            except ValueError as err:
                records.append(failure_record(case, expected, None, str(err)))
                return f"FAIL {case.name} {err}"
            if not outcomes_match(expected, actual):
                records.append(failure_record(case, expected, actual, None))
                return f"FAIL {case.name} expected {expected!r} actual {actual!r}"
            return None

        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
            futures = {pool.submit(one, case): case for case in cases}
            for future in concurrent.futures.as_completed(futures):
                case = futures[future]
                failure = future.result()
                if failure:
                    failures.append(case.name)
                    print(failure, flush=True)
                else:
                    print("PASS", case.name, flush=True)
    print(f"{len(failures)} failures of {len(cases)} cases", flush=True)
    if args.report:
        records.sort(key=lambda record: record["name"])
        args.report.write_text(json.dumps(records, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    if getattr(args, "known_failures", None) is not None:
        return against_known_failures(args, cases, failures)
    return not failures


def against_known_failures(args, cases, failures):
    """Judge a run against a recorded list of cases known to fail, as compat's baseline does: fail
    on a case that fails but is not listed, and on a listed case that now passes, so the list only
    ever shrinks. `--update-known-failures` rewrites the list from this run instead."""
    path = args.known_failures
    if args.update_known_failures:
        path.write_text(json.dumps(sorted(failures), indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"{len(failures)} failing cases recorded in {path.name}", flush=True)
        return True
    known = set(json.loads(path.read_text(encoding="utf-8"))) if path.exists() else set()
    selected = {case.name for case in cases}
    new = sorted(set(failures) - known)
    fixed = sorted((known & selected) - set(failures))
    for name in new:
        print(f"NEW FAILURE {name}", flush=True)
    for name in fixed:
        print(f"NOW PASSES {name} (remove it from {path.name})", flush=True)
    print(f"{len(failures)} failing, {len(known & selected)} known; {len(new)} new, {len(fixed)} fixed", flush=True)
    return not new and not fixed


def redundant_fixture(case, golden):
    """A deliberate expectation the oracle already agrees with only hides future changes."""
    if case.deliberate is not None and golden == case.deliberate[:3]:
        return "its EXPECTED fixture matches the oracle; remove the fixture"
    if case.deliberate_stderr is not None and golden[2] == case.deliberate_stderr[0]:
        return "its EXPECTED_STDERR fixture matches the oracle; remove the fixture"
    return None


def command_check(args, cases):
    fingerprint = oracle_fingerprint()
    loaded = {case.corpus: read_goldens(case.corpus) for case in cases}

    def expectation(case):
        golden, error = golden_for(case, loaded, fingerprint)
        if error:
            return None, error
        redundant = redundant_fixture(case, golden)
        if redundant:
            return None, redundant
        return expected_for(case, golden), None

    return compare_ours(args, cases, expectation)


def command_live(args, cases):
    build_oracle(args.oracle_image)
    oracle = {}

    def report(case, result):
        pass

    needed = [case for case in cases if case.deliberate is None]
    oracle.update(run_oracle(needed, args.oracle_image, args.jobs, report))

    def expectation(case):
        if case.deliberate is not None:
            return case.deliberate[:3], None
        result = oracle.get(case.name)
        if not isinstance(result, tuple):
            return None, result or "oracle did not run"
        return expected_for(case, result), None

    return compare_ours(args, cases, expectation)


def command_stale(args, cases):
    build_oracle(args.oracle_image)
    fingerprint = oracle_fingerprint()
    loaded = {case.corpus: read_goldens(case.corpus) for case in cases}
    stale = []

    def report(case, result):
        golden, error = golden_for(case, loaded, fingerprint)
        if error or result != golden:
            stale.append(case.name)
            print(f"STALE {case.name}: {error or 'oracle now answers ' + repr(result)}", flush=True)

    # A case with a full `deliberate` expectation never compares its golden against a live oracle
    # answer in `check` or `live` either (see `command_live`'s own `needed` filter and
    # `expectation()` above, which both short-circuit on `case.deliberate is not None`) -- its
    # golden is just the oracle snapshot the override was written against, not something the tool
    # is ever judged by. Re-running the oracle for one anyway makes `stale` flag real machines
    # diverging on their own real-world state (a live filesystem's block/inode counts, the current
    # date) as if the case itself had gone stale, when nothing about what it tests has changed.
    needed = [case for case in cases if case.deliberate is None]
    run_oracle(needed, args.oracle_image, args.jobs, report)
    print(f"{len(stale)} stale of {len(needed)} goldens", flush=True)
    return not stale


# --- Coverage -----------------------------------------------------------------------------------

ERROR_WORDS = re.compile(
    r"error|invalid|missing|refus|unsupported|reject|fail|nonexistent|not found|bad |usage|unknown"
)


def load_features():
    spec = importlib.util.spec_from_file_location("features", HERE / "features.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def case_command(case, commands):
    """The command a case is about, from `cmd: ...` or `corpus: cmd ...` in its name."""
    if ":" not in case.name:
        return None
    prefix, rest = case.name.split(":", 1)
    if prefix in commands:
        return prefix
    first = rest.split()[0] if rest.split() else ""
    return first if first in commands else None


def credited(case, features):
    """Every feature a case covers, and any tags it names that are not features."""
    table = features.features()
    unknown = [tag for tag in case.tags if tag not in table and tag not in features.QUALIFIERS]
    found = {tag for tag in case.tags if tag in table}
    found |= features.detect(case.script)
    command = case_command(case, set(features.COMMANDS))
    if command:
        found.add(f"cmd.{command}")
        if "error" in case.tags or ERROR_WORDS.search(case.name):
            found.add(f"cmd.{command}.error")
    return found, unknown


def shell_commands(args):
    """Our shell's builtins (`compgen -b`) and its other commands (`compgen -c`, less the
    builtins and keywords), so the checklist cannot miss a registered command. As in bash, this
    shell's commands are not builtins; a fresh shell has no functions, aliases or `PATH`."""
    with tempfile.TemporaryDirectory(prefix="golem-conformance-") as directory:
        base = pathlib.Path(directory)
        cache = base / "cache.toml"
        cache.write_text('[cache]\ndirectory = "' + str(base / "cache") + '"\n')
        script = "compgen -b; echo --; compgen -c; echo --; compgen -k"
        case = Case("compgen", script, "coverage")
        status, stdout, stderr = run_ours(case, args.wasm.resolve(), args.wasmtime, base, cache)
    if status != 0:
        raise ValueError(f"compgen failed: {stderr!r}")
    builtins, commands, keywords = (set(part.split()) for part in stdout.decode().split("--\n"))
    return builtins, commands - builtins - keywords


def command_coverage(args, cases):
    features = load_features()
    table = features.features()
    counts = {tag: 0 for tag in table}
    problems = []
    for case in cases:
        found, unknown = credited(case, features)
        for tag in unknown:
            problems.append(f"{case.name}: unknown tag {tag!r}")
        for tag in found:
            counts[tag] += 1
    builtins, commands = shell_commands(args)
    for registered, listed, kind in [
        (builtins, set(features.BUILTINS) | set(features.BUILTIN_REPLACEMENTS), "builtin"),
        (
            commands,
            (set(features.COMMANDS) - set(features.BUILTIN_REPLACEMENTS)) | set(features.FIXTURES),
            "command",
        ),
    ]:
        for name in sorted(registered - listed):
            problems.append(f"{kind} `{name}` is registered but not in features.py")
        for name in sorted(listed - registered):
            problems.append(f"{kind} `{name}` is in features.py but not registered")
    gaps = [tag for tag, count in counts.items() if count == 0 and tag not in features.NOT_APPLICABLE]
    width = max(len(tag) for tag in table)
    for tag in sorted(table):
        mark = "n/a" if tag in features.NOT_APPLICABLE else str(counts[tag])
        print(f"{tag:<{width}}  {mark}")
    for problem in problems:
        print("PROBLEM", problem)
    covered = len(table) - len(features.NOT_APPLICABLE) - len(gaps)
    print(f"{covered} of {len(table) - len(features.NOT_APPLICABLE)} features covered; {len(gaps)} gaps")
    for tag in sorted(gaps):
        print("GAP", tag, "-", table[tag])
    return not gaps and not problems


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", nargs="?", default="check", choices=["check", "record", "live", "stale", "coverage"])
    parser.add_argument("--wasm", type=pathlib.Path)
    parser.add_argument("--wasmtime", default="wasmtime")
    parser.add_argument("--oracle-image", default=ORACLE_IMAGE)
    parser.add_argument("--case", action="append", default=[], help="select a case by exact name")
    parser.add_argument("--prefix", action="append", default=[], help="select cases whose name starts with this")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--report", type=pathlib.Path, help="write failures as JSON, for triage")
    parser.add_argument(
        "--tier", choices=["pr", "all"], default="pr",
        help='check: `pr` skips corpora marked TIER = "sweep"; `all` checks every case',
    )
    parser.add_argument(
        "--known-failures", type=pathlib.Path,
        help="check: a JSON list of cases known to fail; fail only on a new "
        "failure or on a listed case that now passes",
    )
    parser.add_argument(
        "--update-known-failures", action="store_true",
        help="check: rewrite the --known-failures file from this run's failures",
    )
    parser.add_argument(
        "--runs", type=int, default=2,
        help="record: run the oracle this many times per case, each in a fresh container, and "
        "refuse to write a golden the runs disagree on (default 2; use 1 for quick local "
        "iteration, skipping the determinism check)",
    )
    args = parser.parse_args(argv)
    cases = select(load_cases(), args)
    if args.command == "check" and args.tier == "pr":
        skipped = sum(case.tier == "sweep" for case in cases)
        cases = [case for case in cases if case.tier != "sweep"]
        if skipped:
            print(f"skipping {skipped} sweep-tier cases; `--tier all` checks them", flush=True)
    if args.command in ("check", "live", "coverage") and args.wasm is None:
        parser.error(f"{args.command} needs --wasm")
    handler = {
        "check": command_check, "record": command_record,
        "live": command_live, "stale": command_stale, "coverage": command_coverage,
    }[args.command]
    return 0 if handler(args, cases) else 1


if __name__ == "__main__":
    sys.exit(main())
