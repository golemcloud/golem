#!/usr/bin/env python3

import argparse
import json
import math
from pathlib import Path
from typing import Any


class ValidationError(Exception):
    pass


def require_object(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValidationError(f"{name} must be an object")
    return value


def require_non_empty_list(value: Any, name: str) -> list[Any]:
    if not isinstance(value, list) or not value:
        raise ValidationError(f"{name} must be a non-empty array")
    return value


def validate_measurements(value: Any, name: str) -> None:
    measurements = require_object(value, name)
    if not measurements:
        raise ValidationError(f"{name} must not be empty")
    for measurement_name, summary_value in measurements.items():
        summary = require_object(summary_value, f"{name}.{measurement_name}")
        for statistic in ("avg", "min", "max"):
            number = summary.get(statistic)
            if not isinstance(number, (int, float)) or not math.isfinite(number):
                raise ValidationError(
                    f"{name}.{measurement_name}.{statistic} must be a finite number"
                )


def validate_results(
    value: Any,
    *,
    expected_commit: str,
    expected_ref: str,
    expected_runner: str,
    expected_benchmarks: int,
) -> dict[str, Any]:
    collection = require_object(value, "input")
    runs = collection.get("runs")
    if not isinstance(runs, list) or len(runs) != 1:
        raise ValidationError("input.runs must contain exactly one suite run")

    run = require_object(runs[0], "run")
    if run.get("suite") != "CI":
        raise ValidationError("run.suite must be CI")
    runner = require_object(run.get("runner"), "run.runner")
    if runner.get("id") != expected_runner:
        raise ValidationError(f"run.runner.id must be {expected_runner}")
    source = require_object(run.get("source"), "run.source")
    if source.get("repository") != "golemcloud/golem":
        raise ValidationError("run.source.repository must be golemcloud/golem")
    if source.get("commitSha") != expected_commit:
        raise ValidationError(f"run.source.commitSha must be {expected_commit}")
    if source.get("ref") != expected_ref:
        raise ValidationError(f"run.source.ref must be {expected_ref}")

    benchmarks = require_non_empty_list(run.get("results"), "run.results")
    if len(benchmarks) != expected_benchmarks:
        raise ValidationError(
            f"run.results must contain {expected_benchmarks} primary benchmarks, "
            f"found {len(benchmarks)}"
        )
    names: set[str] = set()
    for benchmark_index, benchmark_value in enumerate(benchmarks):
        name = f"run.results[{benchmark_index}]"
        benchmark = require_object(benchmark_value, name)
        benchmark_name = benchmark.get("name")
        if not isinstance(benchmark_name, str) or not benchmark_name:
            raise ValidationError(f"{name}.name must be a non-empty string")
        if benchmark_name in names:
            raise ValidationError(f"run.results contains duplicate benchmark {benchmark_name}")
        names.add(benchmark_name)

        configurations = require_non_empty_list(benchmark.get("runs"), f"{name}.runs")
        results = require_non_empty_list(benchmark.get("results"), f"{name}.results")
        if len(results) != len(configurations):
            raise ValidationError(f"{name}.results must contain one result per run configuration")
        for result_index, result_value in enumerate(results):
            result_name = f"{name}.results[{result_index}]"
            result = require_object(result_value, result_name)
            require_object(result.get("run_config"), f"{result_name}.run_config")
            validate_measurements(
                result.get("duration_results"), f"{result_name}.duration_results"
            )

    return run


def main() -> None:
    parser = argparse.ArgumentParser(description="Validate a completed Amp orb benchmark run")
    parser.add_argument("results", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--runner", default="amp-orb-a1.xxlarge")
    parser.add_argument("--expected-benchmarks", type=int, default=11)
    args = parser.parse_args()

    with args.results.open() as file:
        value = json.load(file)
    run = validate_results(
        value,
        expected_commit=args.commit,
        expected_ref=args.ref,
        expected_runner=args.runner,
        expected_benchmarks=args.expected_benchmarks,
    )
    print(
        f"validated {len(run['results'])} benchmarks for "
        f"{run['runner']['id']} at {run['source']['commitSha']}"
    )


if __name__ == "__main__":
    try:
        main()
    except (OSError, json.JSONDecodeError, ValidationError) as error:
        raise SystemExit(f"benchmark result validation failed: {error}") from error
