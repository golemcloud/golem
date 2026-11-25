use super::{test_binary_path, test_binary_profile};
use futures_util::StreamExt;
use std::path::Path;
use std::process::{Output, Stdio};
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
        "golem" => "golem-server.toml",
        other => panic!("unknown service binary {other}"),
    }
}

/// Configures a binary with an empty config file, so only the defaults and `env` apply.
fn command(binary: &str, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> Command {
    // The service binaries are in `target/debug` for every test profile.
    let profile = if binary == "golem" {
        test_binary_profile()
    } else {
        "debug".to_string()
    };
    let path = test_binary_path(&profile, binary);
    std::fs::create_dir_all(cwd.join("config")).unwrap();
    std::fs::write(cwd.join("config").join(config_file_name(binary)), "").unwrap();

    let mut command = Command::new(path);
    command.args(args).current_dir(cwd);
    std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| key.starts_with("GOLEM__") || key.starts_with("GOLEM_ADVANCED__"))
        .for_each(|name| {
            command.env_remove(name);
        });
    command.env_remove("RUST_BACKTRACE");
    command.env_remove("RUST_LIB_BACKTRACE");
    command.envs(env.iter().copied());
    command.kill_on_drop(true);
    command
}

async fn run(binary: &str, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> Output {
    match tokio::time::timeout(RUN_TIMEOUT, command(binary, args, env, cwd).output()).await {
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

#[cfg(unix)]
#[test]
#[timeout("10m")]
async fn server_cleanup_preserves_logs_until_the_final_configuration() {
    use expectrl::Expect;
    use expectrl::process::unix::WaitStatus;

    futures_util::stream::iter(["decline", "address", "logging"])
        .for_each(|scenario| async move {
            tokio::task::spawn_blocking(move || {
                let cwd = TempDir::new().unwrap();
                let data = cwd.path().join("data");
                std::fs::create_dir_all(&data).unwrap();
                let marker = data.join("old-state");
                let log = cwd.path().join("server.log");
                let final_log = cwd.path().join("final.log");
                let config = cwd.path().join("settings.toml");
                let cli_config = cwd.path().join("cli-config");
                std::fs::write(&marker, b"old state").unwrap();
                std::fs::write(&log, b"old log to preserve").unwrap();
                std::fs::write(&final_log, b"old final log").unwrap();
                let settings = |address, file_name| {
                    format!(
                        r#"
                    [logging]
                    file_dir = "."
                    file_name = "{file_name}"
                    file_truncate = true
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = false
                    [advanced.server.router]
                    address = "{address}"
                "#
                    )
                };
                std::fs::write(&config, settings("127.0.0.1", "server.log")).unwrap();
                let mut command = command(
                    "golem",
                    &[
                        "--local",
                        "--config-dir",
                        cli_config.to_str().unwrap(),
                        "--disable-app-manifest-discovery",
                        "--server-no-limit-change",
                        "server",
                        "run",
                        "--config",
                        config.to_str().unwrap(),
                        "--data-dir",
                        data.to_str().unwrap(),
                        "--clean",
                    ],
                    &[],
                    cwd.path(),
                )
                .into_std();
                command.env("HOME", cwd.path());
                command.env("TERM", "xterm-256color");
                command.env("RUST_LOG", "info");
                let mut session = expectrl::Session::spawn(command).unwrap();
                session.set_expect_timeout(Some(RUN_TIMEOUT));
                session
                    .expect("Permanently delete the local server data directory")
                    .unwrap();
                match scenario {
                    "address" => {
                        std::fs::write(&config, settings("0.0.0.0", "server.log")).unwrap();
                        session.send_line("y").unwrap();
                        session
                            .expect(
                                "Refusing to listen on addresses that are not loopback addresses",
                            )
                            .unwrap();
                    }
                    "logging" => {
                        std::fs::write(&config, settings("127.0.0.1", "final.log")).unwrap();
                        session.send_line("y").unwrap();
                        session.expect("Tracing initialized").unwrap();
                        session.send(expectrl::ControlCode::ETX).unwrap();
                    }
                    _ => session.send_line("n").unwrap(),
                }
                session.expect(expectrl::Eof).unwrap();
                let status = session.get_process().wait().unwrap();
                if scenario != "logging" {
                    assert!(matches!(status, WaitStatus::Exited(_, 1)), "{status:?}");
                    assert_eq!(std::fs::read(&final_log).unwrap(), b"old final log");
                } else {
                    let contents = std::fs::read_to_string(&final_log).unwrap();
                    assert!(contents.contains("Tracing initialized"), "{contents}");
                    assert!(!contents.contains("old final log"));
                }
                assert_eq!(std::fs::read(&log).unwrap(), b"old log to preserve");
                if scenario != "decline" {
                    assert!(!marker.exists(), "confirmed cleanup removes the old state");
                } else {
                    assert_eq!(std::fs::read(&marker).unwrap(), b"old state");
                }
            })
            .await
            .unwrap();
        })
        .await;
}

async fn assert_config_exit_codes(binary: &str, setting: &str) {
    let cwd = TempDir::new().unwrap();
    // 1. A setting that does not load: exit code 1, the error printed once.
    let output = run(binary, &[], &[(setting, "notanumber")], cwd.path()).await;
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
        cwd.path(),
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
    let output = run(binary, &["--dump-config-bogus"], &[], cwd.path()).await;
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
    assert_config_exit_codes(
        "worker-executor",
        "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT",
    )
    .await;
}

#[test]
#[timeout("6m")]
async fn worker_service_config_exit_codes() {
    assert_config_exit_codes(
        "golem-worker-service",
        "GOLEM_ADVANCED__WORKER_SERVICE__PORT",
    )
    .await;
}

#[test]
#[timeout("6m")]
async fn registry_service_config_exit_codes() {
    assert_config_exit_codes(
        "golem-registry-service",
        "GOLEM_ADVANCED__REGISTRY_SERVICE__GRPC__PORT",
    )
    .await;
}

#[test]
#[timeout("6m")]
async fn shard_manager_config_exit_codes() {
    assert_config_exit_codes(
        "golem-shard-manager",
        "GOLEM_ADVANCED__SHARD_MANAGER__GRPC__PORT",
    )
    .await;
}

#[test]
#[timeout("6m")]
async fn component_compilation_service_config_exit_codes() {
    assert_config_exit_codes(
        "golem-component-compilation-service",
        "GOLEM_ADVANCED__COMPONENT_COMPILATION_SERVICE__GRPC__PORT",
    )
    .await;
}

#[test]
#[timeout("20m")]
async fn unreachable_s3_fails_startup_without_local_blob_storage() {
    let registry = TempDir::new().unwrap();
    let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let grpc = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = http.local_addr().unwrap().port();
    let grpc_port = grpc.local_addr().unwrap().port().to_string();
    drop((http, grpc));
    let data_dir = registry.path().join("data");
    let log_path = registry.path().join("registry.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut registry_process = command(
        "golem-registry-service",
        &[],
        &[
            ("GOLEM__DATA_DIR", data_dir.to_str().unwrap()),
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__HTTP_PORT",
                &http_port.to_string(),
            ),
            ("GOLEM_ADVANCED__REGISTRY_SERVICE__GRPC__PORT", &grpc_port),
            (
                "GOLEM_ADVANCED__REGISTRY_SERVICE__COMPONENT_COMPILATION__TYPE",
                "Disabled",
            ),
        ],
        registry.path(),
    )
    .stdout(Stdio::from(log.try_clone().unwrap()))
    .stderr(Stdio::from(log))
    .spawn()
    .unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let ready = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        Duration::from_millis(100),
    ))
    .then(|_| {
        http.get(format!("http://127.0.0.1:{http_port}/healthcheck"))
            .send()
    })
    .filter(|response| {
        std::future::ready(
            response
                .as_ref()
                .is_ok_and(|response| response.status().is_success()),
        )
    });
    futures_util::pin_mut!(ready);
    tokio::time::timeout(RUN_TIMEOUT, ready.next())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "registry prerequisite did not start: {error}\n{}",
                std::fs::read_to_string(&log_path).unwrap()
            )
        })
        .expect("readiness stream ended")
        .expect("registry healthcheck failed");

    let grpc_port = grpc_port.as_str();
    futures_util::stream::iter([
        ("golem-component-compilation-service", &[][..]),
        ("golem-registry-service", &[][..]),
        ("golem-worker-service", &[][..]),
        ("worker-executor", &[][..]),
        (
            "golem",
            &[
                "server",
                "run",
                "--config",
                "config/golem-server.toml",
                "--router-port",
                "0",
                "--custom-request-port",
                "0",
                "--mcp-port",
                "0",
            ][..],
        ),
    ])
    .for_each(|(binary, args)| async move {
        let cwd = TempDir::new().unwrap();
        let data = cwd.path().join("data");
        let blobs = cwd.path().join("local-blobs");
        let output = run(
            binary,
            args,
            &[
                ("GOLEM__DATA_DIR", data.to_str().unwrap()),
                ("GOLEM__DATA_DISK__BLOB_ROOT", blobs.to_str().unwrap()),
                ("GOLEM__BLOB_STORE", "s3"),
                ("GOLEM__OBJECT_STORAGE__ENDPOINT_URL", "http://127.0.0.1:1"),
                ("GOLEM__OBJECT_STORAGE__PATH_STYLE", "true"),
                ("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", "false"),
                (
                    "GOLEM_ADVANCED__WORKER_SERVICE__REGISTRY_SERVICE__HOST",
                    "127.0.0.1",
                ),
                (
                    "GOLEM_ADVANCED__WORKER_SERVICE__REGISTRY_SERVICE__PORT",
                    grpc_port,
                ),
                (
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__REGISTRY_SERVICE__HOST",
                    "127.0.0.1",
                ),
                (
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__REGISTRY_SERVICE__PORT",
                    grpc_port,
                ),
                ("GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT", "0"),
                ("GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT", "0"),
                ("AWS_ACCESS_KEY_ID", "test-access-key"),
                ("AWS_SECRET_ACCESS_KEY", "test-secret-key"),
                ("AWS_EC2_METADATA_DISABLED", "true"),
            ],
            cwd.path(),
        )
        .await;
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            describe(binary, &output)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("S3 startup check"),
            "{}",
            describe(binary, &output)
        );
        assert!(
            !blobs.exists(),
            "{} created its designated local blob root",
            describe(binary, &output)
        );
        assert!(
            !data.join("blobs").exists(),
            "{} created the default local blob root",
            describe(binary, &output)
        );
    })
    .await;
    registry_process.kill().await.unwrap();
    registry_process.wait().await.unwrap();
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
