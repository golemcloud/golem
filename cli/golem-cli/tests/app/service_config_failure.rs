use super::{test_binary_path, test_binary_profile};
use std::process::Output;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{test, timeout};
use tokio::process::Command;

/// How long one run of a binary may take. A binary that fails to load its config exits at once,
/// but the first start of a freshly built debug binary can take about a minute on macOS, because
/// the system checks a new executable when it first runs.
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

/// The file that a service binary reads from `config/` in its working directory. The loader
/// requires the file to exist.
fn config_file_name(binary: &str) -> &'static str {
    match binary {
        "worker-executor" => "worker-executor.toml",
        "golem-worker-service" => "worker-service.toml",
        "golem-registry-service" => "registry-service.toml",
        "golem-shard-manager" => "shard-manager.toml",
        "golem-component-compilation-service" => "component-compilation-service.toml",
        other => panic!("unknown service binary {other}"),
    }
}

/// Runs a service binary in a directory with an empty config file, so only the defaults and
/// `env` apply.
async fn run(binary: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    // The service binaries are in `target/debug` for every test profile.
    let path = test_binary_path("debug", binary);
    let cwd = TempDir::new().unwrap();
    std::fs::create_dir(cwd.path().join("config")).unwrap();
    std::fs::write(cwd.path().join("config").join(config_file_name(binary)), "").unwrap();

    let mut command = Command::new(path);
    command.args(args).current_dir(cwd.path());
    for (name, _) in std::env::vars_os().filter_map(|(k, v)| k.into_string().ok().map(|k| (k, v))) {
        if name.starts_with("GOLEM__") {
            command.env_remove(name);
        }
    }
    command.env_remove("RUST_BACKTRACE");
    command.env_remove("RUST_LIB_BACKTRACE");
    for (name, value) in env {
        command.env(name, value);
    }
    command.kill_on_drop(true);

    match tokio::time::timeout(RUN_TIMEOUT, command.output()).await {
        Ok(output) => output.unwrap_or_else(|err| panic!("failed to run {binary}: {err}")),
        Err(_) => panic!("{binary} did not exit within {RUN_TIMEOUT:?}"),
    }
}

fn describe(binary: &str, output: &Output) -> String {
    format!(
        "{binary}: status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

async fn assert_config_exit_codes(binary: &str, setting: &str) {
    // 1. A setting that does not load: exit code 1, the error printed once.
    let output = run(binary, &[], &[(setting, "notanumber")]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        describe(binary, &output)
    );
    assert_eq!(
        stderr.matches("Failed to load config").count(),
        1,
        "{}",
        describe(binary, &output)
    );
    assert!(
        stderr.contains("notanumber"),
        "{}",
        describe(binary, &output)
    );

    // 2. A dump flag does not load the configuration, so a bad setting does not matter.
    let output = run(
        binary,
        &["--dump-config-default-toml"],
        &[(setting, "notanumber")],
    )
    .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        describe(binary, &output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("## Generated from default config"),
        "{}",
        describe(binary, &output)
    );

    // 3. An unknown dump flag: exit code 1 and a message, not a panic.
    let output = run(binary, &["--dump-config-bogus"], &[]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        describe(binary, &output)
    );
    assert!(
        stderr.contains("Unknown dump config parameter: --dump-config-bogus"),
        "{}",
        describe(binary, &output)
    );
    assert!(
        !stderr.contains("panicked"),
        "{}",
        describe(binary, &output)
    );
}

#[test]
#[timeout("6m")]
async fn worker_executor_config_exit_codes() {
    assert_config_exit_codes("worker-executor", "GOLEM__GRPC__PORT").await;
}

#[test]
#[timeout("6m")]
async fn worker_service_config_exit_codes() {
    assert_config_exit_codes("golem-worker-service", "GOLEM__PORT").await;
}

#[test]
#[timeout("6m")]
async fn registry_service_config_exit_codes() {
    assert_config_exit_codes("golem-registry-service", "GOLEM__GRPC__PORT").await;
}

#[test]
#[timeout("6m")]
async fn shard_manager_config_exit_codes() {
    assert_config_exit_codes("golem-shard-manager", "GOLEM__GRPC__PORT").await;
}

#[test]
#[timeout("6m")]
async fn component_compilation_service_config_exit_codes() {
    assert_config_exit_codes("golem-component-compilation-service", "GOLEM__GRPC__PORT").await;
}

#[test]
#[timeout("6m")]
async fn golem_rejects_an_unknown_dump_flag() {
    let binary = "golem";
    let path = test_binary_path(&test_binary_profile(), binary);
    let cwd = TempDir::new().unwrap();
    let mut command = Command::new(path);
    command
        .args(["--dump-config-bogus"])
        .current_dir(cwd.path())
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .kill_on_drop(true);
    let output = match tokio::time::timeout(RUN_TIMEOUT, command.output()).await {
        Ok(output) => output.unwrap_or_else(|err| panic!("failed to run {binary}: {err}")),
        Err(_) => panic!("{binary} did not exit within {RUN_TIMEOUT:?}"),
    };

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        describe(binary, &output)
    );
    assert!(
        text.contains("Unknown dump config parameter"),
        "{}",
        describe(binary, &output)
    );
    assert!(!text.contains("panicked"), "{}", describe(binary, &output));
}
