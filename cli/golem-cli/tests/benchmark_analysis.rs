// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

#[cfg(unix)]
use test_r::{test, timeout};
#[cfg(unix)]
use tokio::process::Command;

#[cfg(unix)]
#[test]
#[timeout("15m")]
async fn paired_benchmark_cli_preserves_order_failures_and_timeout_evidence() {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    // CI restores this command with its service-test binaries. Local targeted runs
    // can build it offline when it has not been prepared yet.
    let prebuilt = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::workspace_path().join("target"))
        .join("debug/benchmark-results");
    let binary = if prebuilt.is_file() {
        prebuilt
    } else {
        let build = Command::new("cargo")
            .kill_on_drop(true)
            .current_dir(crate::workspace_path())
            .args([
                "build",
                "--locked",
                "--offline",
                "-p",
                "integration-tests",
                "--bin",
                "benchmark-results",
                "--message-format=json",
            ])
            .output()
            .await
            .expect("build the real benchmark-results command");
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        String::from_utf8(build.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|message| {
                message["target"]["name"] == "benchmark-results"
                    && message["executable"].is_string()
            })
            .unwrap()["executable"]
            .as_str()
            .unwrap()
            .into()
    };

    fn executable(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    for environment in [
        None,
        Some(
            json!({"runner_id": "deployment", "hardware_fingerprint": "aarch64-cpu",
            "storage_profile": "postgres"}),
        ),
        Some(
            json!({"runner_id": "deployment", "hardware_fingerprint": "aarch64-cpu",
            "storage_profile": "postgres", "sockets_enabled": false}),
        ),
    ] {
        let mut command = Command::new(&binary);
        command.arg("validate-environment");
        if let Some(value) = &environment {
            command.env("GOLEM_BENCH_MEASUREMENT_ENVIRONMENT", value.to_string());
        } else {
            command.env_remove("GOLEM_BENCH_MEASUREMENT_ENVIRONMENT");
        }
        let output = command.output().await.unwrap();
        let complete = environment
            .as_ref()
            .is_some_and(|value| value.get("sockets_enabled").is_some());
        assert_eq!(output.status.success(), complete);
        if !complete {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("GOLEM_BENCH_MEASUREMENT_ENVIRONMENT"),
                "{error}"
            );
            assert!(!error.contains("panicked"), "{error}");
        }
    }

    for (mode, pair_count) in [
        ("success", 3),
        ("success", 4),
        ("drift", 4),
        ("slowdown", 4),
        ("speedup", 4),
        ("failure", 3),
        ("timeout", 3),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(&bin.join("git"), "#!/bin/sh\ncat source-sha\n");
        executable(&bin.join("docker"), "#!/bin/sh\nexit 0\n");
        for side in ["base", "head"] {
            let checkout = root.join(side);
            std::fs::create_dir_all(checkout.join("target/benchmarks")).unwrap();
            std::fs::write(checkout.join("source-sha"), side).unwrap();
            std::fs::write(checkout.join("builtin-artifacts.json"), "{}").unwrap();
        }
        let harness = root.join("head/target/benchmarks/benchmarks");
        executable(
            &harness,
            r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
    case "$1" in
        --run-id) id="$2"; shift ;;
        --source-commit-sha) source="$2"; shift ;;
        --save-to-json) output="$2"; shift ;;
    esac
    shift
done
echo "$source" >> "$DUMMY_ROOT/launches"
echo "dummy $source"
if [ "$source" = head ]; then
    case "$DUMMY_MODE" in
        failure) exit 17 ;;
        timeout)
            sleep 30 &
            echo "$!" > "$DUMMY_ROOT/sleeper.pid"
            wait
            exit 19
            ;;
    esac
fi
value=20
case "$DUMMY_MODE" in
    drift) value=$((100 + $(wc -l < "$DUMMY_ROOT/launches"))) ;;
    slowdown) [ "$source" != head ] || value=40 ;;
    speedup) [ "$source" != head ] || value=10 ;;
esac
sed -e "s/PROCESS_ID/$id/g" -e "s/SOURCE_SHA/$source/g" -e "s/METRIC_VALUE/$value/g" "$DUMMY_ROOT/record.json" > "$output"
"#,
        );
        let hash = format!("{:x}", Sha256::digest(std::fs::read(&harness).unwrap()));
        let identity = json!([
            1,
            1,
            "dummy-runner",
            "dummy-hardware",
            "dummy-storage",
            false
        ]);
        let series = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&identity).unwrap())
        );
        let config =
            json!({"clusterSize": 1, "size": 7, "length": 13, "disableCompilationCache": false});
        let record = json!({"runs": [{
            "schema_version": 2, "harness_version": 1, "suite": "dummy",
            "suite_fingerprint": "dummy-suite", "environment": "dummy", "version": "dummy",
            "timestamp": "2026-01-01T00:00:00Z", "run_id": "PROCESS_ID",
            "runner": {"id": "dummy-runner"},
            "source": {"repository": "golemcloud/golem", "commitSha": "SOURCE_SHA"},
            "artifacts": {"runner": {"path": harness, "sha256": hash}, "services": {}, "fixtures": {}},
            "results": [{"name": "dummy", "description": "dummy", "entry_version": 1,
                "iterations": 1, "series_key": series,
                "measurement_environment": {"runner_id": "dummy-runner", "hardware_fingerprint": "dummy-hardware",
                    "storage_profile": "dummy-storage", "sockets_enabled": false},
                "runs": [config], "results": [{"run_config": config, "iteration_results": [{
                    "index": 0, "duration_samples": {"latency": ["METRIC_VALUE"]},
                    "count_samples": {}, "failures": {}}]}]}]
        }]});
        std::fs::write(
            root.join("record.json"),
            serde_json::to_string(&record)
                .unwrap()
                .replace("\"METRIC_VALUE\"", "METRIC_VALUE"),
        )
        .unwrap();
        std::fs::write(
            root.join("suite.yaml"),
            "dummy suite; not a benchmark workload",
        )
        .unwrap();
        let evidence = root.join("evidence");
        let output = Command::new(&binary)
            .kill_on_drop(true)
            .env("GITHUB_ACTIONS", "true")
            .env("DUMMY_ROOT", root)
            .env("DUMMY_MODE", mode)
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(bin)
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            )
            .arg("paired")
            .arg("--base-root")
            .arg(root.join("base"))
            .arg("--head-root")
            .arg(root.join("head"))
            .args(["--base-source", "base", "--head-source", "head"])
            .arg("--suite")
            .arg(root.join("suite.yaml"))
            .args([
                "--pairs",
                &pair_count.to_string(),
                "--runner-id",
                "dummy-runner",
                "--timeout-secs",
                if mode == "timeout" { "3" } else { "30" },
            ])
            .arg("--output")
            .arg(&evidence)
            .output()
            .await
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let launches = std::fs::read_to_string(root.join("launches")).unwrap();
        let plan: Value =
            serde_json::from_slice(&std::fs::read(evidence.join("plan.json")).unwrap()).unwrap();
        let orders = &[
            ["base", "head"],
            ["head", "base"],
            ["base", "head"],
            ["head", "base"],
        ][..pair_count];
        assert_eq!(plan["order"], json!(orders));
        assert!(evidence.join("0-base.json").is_file());
        assert_eq!(
            std::fs::read_to_string(evidence.join("0-head.log")).unwrap(),
            "dummy head\n"
        );
        if !matches!(mode, "failure" | "timeout") {
            assert!(output.status.success(), "{stderr}");
            assert_eq!(
                launches,
                orders
                    .iter()
                    .flatten()
                    .map(|side| format!("{side}\n"))
                    .collect::<String>()
            );
            let report: Value =
                serde_json::from_slice(&std::fs::read(evidence.join("comparison.json")).unwrap())
                    .unwrap();
            assert_eq!(report["process_pairs"], pair_count);
            let expected = match mode {
                "slowdown" => 1.0,
                "speedup" => -0.5,
                // The middle paired changes under linear clock drift are -1/108 and 1/105.
                "drift" => (-1.0 / 108.0 + 1.0 / 105.0) / 2.0,
                _ => 0.0,
            };
            let change = report["results"][0]["relative_change"].as_f64().unwrap();
            assert!(
                (change - expected).abs() < 1e-12,
                "{mode}: {change} != {expected}"
            );
            if mode == "success" {
                assert_eq!(
                    report["results"][0]["confidence_interval"],
                    json!([0.0, 0.0])
                );
            }
            for (pair, order) in orders.iter().enumerate() {
                for side in ["base", "head"] {
                    let prefix = format!("{pair}-{side}");
                    let status: Value = serde_json::from_slice(
                        &std::fs::read(evidence.join(format!("{prefix}.status.json"))).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(status["success"], true);
                    assert_eq!(status["pair"], pair);
                    assert_eq!(status["side"], side);
                    assert_eq!(
                        status["position"],
                        order.iter().position(|item| *item == side).unwrap()
                    );
                    assert!(evidence.join(format!("{prefix}.json")).is_file());
                }
            }
        } else {
            assert!(!output.status.success());
            assert_eq!(
                launches, "base\nhead\n",
                "no replacement or next pair after {mode}"
            );
            assert!(!evidence.join("comparison.json").exists());
            if mode == "failure" {
                let status: Value = serde_json::from_slice(
                    &std::fs::read(evidence.join("0-head.status.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(status["exit_code"], 17);
                assert_eq!(status["success"], false);
            } else {
                assert!(stderr.contains("budget expired"), "{stderr}");
                let pid = std::fs::read_to_string(root.join("sleeper.pid")).unwrap();
                let mut running = true;
                for _ in 0..20 {
                    let state = Command::new("ps")
                        .args(["-o", "stat=", "-p", pid.trim()])
                        .output()
                        .await
                        .unwrap();
                    let state = String::from_utf8_lossy(&state.stdout);
                    // A killed child can briefly remain a zombie until the system reaps it.
                    running = !state.trim().is_empty() && !state.trim().starts_with('Z');
                    if !running {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if running {
                    Command::new("kill")
                        .args(["-KILL", pid.trim()])
                        .status()
                        .await
                        .unwrap();
                }
                assert!(!running, "paired timeout left its dummy grandchild running");
            }
        }
    }
}
