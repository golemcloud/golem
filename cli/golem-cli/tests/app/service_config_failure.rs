use super::{test_binary_path, test_binary_profile};
use futures_util::StreamExt;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
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
    let profile = if binary == "golem" {
        test_binary_profile()
    } else {
        std::env::var("GOLEM_TEST_BIN_PROFILE").unwrap_or_else(|_| "debug".to_string())
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
                        "server",
                        "run",
                        "--config",
                        config.to_str().unwrap(),
                        "--data-dir",
                        data.to_str().unwrap(),
                        "--clean",
                    ],
                    &[("GOLEM_SERVER_NO_LIMIT_CHANGE", "true")],
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
                        let deadline = std::time::Instant::now() + RUN_TIMEOUT;
                        std::iter::repeat_with(|| {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "the file log was not initialized"
                            );
                            let ready = std::fs::read_to_string(&final_log)
                                .is_ok_and(|contents| contents.contains("Tracing initialized"));
                            if !ready {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            ready
                        })
                        .find(|ready| *ready)
                        .unwrap();
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
    std::fs::create_dir(&data_dir).unwrap();
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
                "--config-dir",
                "isolated-cli",
                "--disable-app-manifest-discovery",
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
        let fixture = TempDir::new().unwrap();
        let cwd = fixture.path().join("cwd").into_boxed_path();
        std::fs::create_dir(&cwd).unwrap();
        let adapter_default =
            golem_cli::fs::normalize_path_lexically(&cwd.join("../data/blob_storage"))
                .into_boxed_path();
        assert!(
            adapter_default.starts_with(fixture.path()),
            "adapter default escaped the fixture: {}",
            adapter_default.display()
        );
        let data = cwd.join("data").into_boxed_path();
        std::fs::create_dir(&data).unwrap();
        let blobs = cwd.join("local-blobs").into_boxed_path();
        let output = run(
            binary,
            args,
            &[
                ("HOME", cwd.to_str().unwrap()),
                ("GOLEM__STATE_STORE", "sqlite"),
                ("GOLEM__CACHE__ENABLED", "false"),
                ("GOLEM__DATA_DIR", data.to_str().unwrap()),
                ("GOLEM__DATA_DISK__BLOB_ROOT", blobs.to_str().unwrap()),
                ("GOLEM__BLOB_STORE", "s3"),
                ("GOLEM__OBJECT_STORAGE__ENDPOINT_URL", "http://127.0.0.1:1"),
                ("GOLEM__OBJECT_STORAGE__PATH_STYLE", "true"),
                ("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", "false"),
                ("GOLEM__LOGGING__FILE__ENABLED", "true"),
                ("GOLEM__LOGGING__FILE_DIR", data.to_str().unwrap()),
                ("GOLEM__LOGGING__FILE_NAME", "startup-refusal.log"),
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
            ]
            .into_iter()
            .filter(|(key, _)| binary != "golem" || !key.starts_with("GOLEM_ADVANCED__"))
            .collect::<Vec<_>>(),
            &cwd,
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
            "{} created the settings-derived local blob root",
            describe(binary, &output)
        );
        assert!(
            !adapter_default.exists(),
            "{} created the adapter default local blob root at {}",
            describe(binary, &output),
            adapter_default.display()
        );
        let state_databases: &[&str] = match binary {
            "golem-component-compilation-service" => &[],
            "golem-registry-service" => &["registry.db"],
            "golem-worker-service" => &["gateway-sessions.db"],
            "worker-executor" => &["scheduler.db"],
            "golem" => &[
                "registry.db",
                "shard_manager.db",
                "gateway-sessions.db",
                "scheduler.db",
            ],
            other => panic!("unknown S3 consumer {other}"),
        };
        let allowed_files = state_databases
            .iter()
            .flat_map(|db| ["", "-wal", "-shm"].map(|suffix| data.join(format!("{db}{suffix}"))))
            .chain([
                data.join("filesystem-snapshots.repository-key"),
                data.join(if binary == "worker-executor" {
                    "startup-refusal.0.log"
                } else {
                    "startup-refusal.log"
                }),
            ])
            .map(|path| path.into_boxed_path())
            .collect::<Box<[_]>>();
        walkdir::WalkDir::new(&data).into_iter().for_each(|entry| {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                assert!(
                    allowed_files.iter().any(|path| path.as_ref() == entry.path()),
                    "unexpected file after {binary} refused S3: {}",
                    entry.path().display()
                );
            }
        });
        futures_util::stream::iter(
            state_databases
                .iter()
                .map(|db| data.join(db).into_boxed_path()),
        )
            .filter(|database| std::future::ready(database.is_file()))
            .for_each(|database| async move {
                let pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect_with(
                        SqliteConnectOptions::new()
                            .filename(&database)
                            .read_only(true)
                            .create_if_missing(false),
                    )
                    .await
                    .unwrap_or_else(|error| {
                        panic!("cannot inspect {binary} state at {}: {error}", database.display())
                    });
                let blob_tables: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'blob_storage' COLLATE NOCASE",
                )
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| {
                    panic!("cannot inspect {binary} schema at {}: {error}", database.display())
                });
                pool.close().await;
                assert_eq!(
                    blob_tables,
                    0,
                    "{binary} initialized SQLite blob storage at {} before refusing S3",
                    database.display()
                );
            })
            .await;
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

#[cfg(unix)]
#[test]
#[timeout("3m")]
async fn litestream_refuses_postgres_before_overrides_and_restore() {
    use std::os::unix::fs::PermissionsExt;

    let entrypoint =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../golem/docker/golem-entrypoint.sh");
    let postgres = "host='db.invalid'\nport=5432\ndatabase='golem'\nusername='synthetic-user'\npassword='synthetic-private-marker'\nmax_connections=2\nschema='litestream_test'\n";
    futures_util::stream::iter([
        ("sqlite", "", true),
        ("postgres", "", false),
        ("sqlite", "worker_executor.indexed_storage", false),
        ("sqlite", "worker_executor.key_value_storage", false),
        ("sqlite", "worker_executor.key_value_storage.config.cache", false),
        ("sqlite", "worker_executor.key_value_storage.config.persistent", false),
        ("sqlite", "worker_service.mcp.session_storage", false),
    ].into_iter().flat_map(|case| [false, true].map(|existing| (case, existing))))
    .for_each(|((state, selected, allowed), existing)| {
        let entrypoint = &entrypoint;
        async move {
            let cwd = TempDir::new().unwrap();
            let bin = cwd.path().join("bin");
            let data = cwd.path().join("new state");
            let calls = cwd.path().join("litestream-calls");
            let file = cwd.path().join("settings with 'quotes'.toml");
            let log = cwd.path().join("must-not-truncate.log");
            let ports = cwd.path().join("ports.json");
            std::fs::write(&log, b"existing log").unwrap();
            if existing {
                std::fs::create_dir(&data).unwrap();
                std::fs::write(data.join("old-state"), b"existing state").unwrap();
                std::fs::write(data.join("filesystem-snapshots.repository-key"), b"existing key").unwrap();
            }
            std::fs::create_dir(&bin).unwrap();
            std::fs::write(bin.join("golem"), r#"#!/bin/bash
if [[ "$1" != --litestream-preflight ]]; then
  echo unexpected-server-launch >&2
  exit 91
fi
[[ -z "${GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__TYPE:-}" ]] || exit 92
[[ -z "${GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE:-}" ]] || exit 93
exec "$GOLEM_REAL" "$@"
"#).unwrap();
            std::fs::write(bin.join("litestream"), r#"#!/bin/bash
printf '%s\n' "$*" >> "$LITESTREAM_CALLS"
if [[ "$1" == databases ]]; then printf 'path replica\n'; fi
"#).unwrap();
            ["golem", "litestream"].into_iter().for_each(|name| {
                std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
            });
            let advanced = if selected.is_empty() {
                String::new()
            } else if let Some(inner) = selected.strip_prefix("worker_executor.key_value_storage.config.") {
                let other = if inner == "cache" { "persistent" } else { "cache" };
                format!("[advanced.worker_executor.key_value_storage]\ntype='NamespaceRouted'\n[advanced.{selected}]\ntype='Postgres'\n[advanced.{selected}.config]\n{postgres}\n[advanced.worker_executor.key_value_storage.config.{other}]\ntype='InMemory'\n[advanced.worker_executor.key_value_storage.config.{other}.config]\n")
            } else {
                format!("[advanced.{selected}]\ntype='Postgres'\n[advanced.{selected}.config]\n{}{}", if selected.contains("mcp") { "cleanup_interval='5m'\n" } else { "" }, postgres)
            };
            std::fs::write(&file, format!(
                "state_store='{state}'\nblob_store='data_disk'\n[cache]\nenabled=false\n[filesystem_snapshots]\nenabled=true\n[logging]\nfile_dir='.'\nfile_name='must-not-truncate.log'\nfile_truncate=true\n[logging.file]\nenabled=true\n{advanced}"
            )).unwrap();
            let mut command = Command::new("bash");
            command.arg(entrypoint).args(["--config-dir", "isolated-cli", "server", "run"]);
            if existing { command.arg(format!("--config={}", file.display())); }
            else { command.arg("--config").arg(&file); }
            command.arg("--clean").arg("--ports-file").arg(&ports)
                .current_dir(cwd.path()).env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", cwd.path())
                .env("GOLEM_REAL", test_binary_path(&test_binary_profile(), "golem"))
                .env("GOLEM__DATA_DIR", &data)
                .env("GOLEM_LITESTREAM", "true")
                .env("GOLEM_LITESTREAM_REPLICA_URL", "file:///unused-replica")
                .env("LITESTREAM_CALLS", &calls).kill_on_drop(true);
            let output = tokio::time::timeout(RUN_TIMEOUT, command.output()).await.unwrap().unwrap();
            let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(!text.contains("synthetic-private-marker"));
            if allowed {
                assert!(output.status.success(), "{}", describe("entrypoint", &output));
                assert!(std::fs::read_to_string(&calls).unwrap().contains("replicate"));
            } else {
                assert_eq!(output.status.code(), Some(1), "{}", describe("entrypoint", &output));
                assert!(text.contains("GOLEM_LITESTREAM=true") && text.contains("PostgreSQL"), "{text}");
                assert!(!calls.exists(), "Litestream ran before PostgreSQL refusal");
                assert_eq!(data.exists(), existing, "preflight created the state directory");
            }
            assert_eq!(std::fs::read(&log).unwrap(), b"existing log");
            assert!(!ports.exists());
            if existing {
                assert_eq!(std::fs::read(data.join("old-state")).unwrap(), b"existing state");
                assert_eq!(std::fs::read(data.join("filesystem-snapshots.repository-key")).unwrap(), b"existing key");
            } else {
                assert!(!data.join("filesystem-snapshots.repository-key").exists());
            }
        }
    }).await;
    futures_util::stream::iter([
        &["--help"][..],
        &["--version"][..],
        &["server", "clean"][..],
        &["server", "run", "--help"][..],
        &["server", "run", "--not-real"][..],
    ])
    .for_each(|args| async move {
        let cwd = TempDir::new().unwrap();
        let output = Command::new(test_binary_path(&test_binary_profile(), "golem"))
            .arg("--litestream-preflight")
            .args(args)
            .current_dir(cwd.path())
            .env_clear()
            .env("HOME", cwd.path())
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            describe("preflight", &output)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("GOLEM_LITESTREAM=true requires a valid `server run` command")
        );
        assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
    })
    .await;
}
