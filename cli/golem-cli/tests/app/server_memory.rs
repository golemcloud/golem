use super::{test_binary_path, test_binary_profile};
use crate::{Tracing, workspace_path};
use indoc::indoc;
use std::process::Stdio;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::process::Command;

inherit_test_dep!(Tracing);

#[test]
async fn local_server_memory_budget_precedence_and_validation() {
    let root = workspace_path().join("tmp");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let manifest = directory.path().join("golem.yaml");
    std::fs::write(
        &manifest,
        indoc! {"
        manifestVersion: '1.6.0'
        app: memory-budget-test
        environments:
          local:
            default: true
            server: local
        localServer:
          memoryBudget: 2147483648
    "},
    )
    .unwrap();
    let binary = test_binary_path(&test_binary_profile(), "golem");
    for (index, (env, flag, expected)) in [
        (None, None, 2147483648u64),
        (Some("1073741824"), None, 1073741824),
        (Some("invalid"), Some("3221225472"), 3221225472),
    ]
    .into_iter()
    .enumerate()
    {
        let ports = directory.path().join(format!("ports-{index}.json"));
        let log_path = directory.path().join(format!("server-{index}.log"));
        let log = std::fs::File::create(&log_path).unwrap();
        let mut command = Command::new(&binary);
        command.args([
            "--app-manifest-path",
            manifest.to_str().unwrap(),
            "server",
            "run",
            "--router-addr",
            "127.0.0.1",
            "--router-port",
            "0",
            "--custom-request-port",
            "0",
            "--mcp-port",
            "0",
            "--ports-file",
            ports.to_str().unwrap(),
            "--data-dir",
            directory.path().join("data").to_str().unwrap(),
        ]);
        command.env_remove("GOLEM_LOCAL_SERVER_MEMORY_BUDGET");
        if let Some(value) = env {
            command.env("GOLEM_LOCAL_SERVER_MEMORY_BUDGET", value);
        }
        if let Some(value) = flag {
            command.args(["--memory-budget", value]);
        }
        let mut child = command
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), async {
            while !ports.exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "Server exited {status}: {}",
                        std::fs::read_to_string(&log_path).unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("server readiness timeout");
        let output = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            output.contains("limit pinned by system_memory_override"),
            "{output}"
        );
        assert!(
            output.contains(&format!("limit_bytes: {expected}")),
            "{output}"
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
    for (env, flag) in [
        (Some("0"), None),
        (Some("invalid"), None),
        (None, Some("0")),
        (None, Some("-1")),
    ] {
        let mut command = Command::new(&binary);
        command.args([
            "--app-manifest-path",
            manifest.to_str().unwrap(),
            "server",
            "run",
        ]);
        command.env_remove("GOLEM_LOCAL_SERVER_MEMORY_BUDGET");
        if let Some(value) = env {
            command.env("GOLEM_LOCAL_SERVER_MEMORY_BUDGET", value);
        }
        if let Some(value) = flag {
            command.arg(format!("--memory-budget={value}"));
        }
        command.kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        let output = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.contains("MEMORY_BUDGET") || output.contains("--memory-budget"),
            "{output}"
        );
    }
}
