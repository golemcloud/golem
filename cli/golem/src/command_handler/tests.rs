// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use golem_cli::log::{LogOutput, OutputCapture, discard_held_log, release_held_log};
use golem_cli::model::app_raw::LocalServer;
use golem_cli::model::format::Format;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::test;
use tracing_subscriber::{Layer, layer::SubscriberExt};

fn local_server(value: LocalServer) -> ResolvedLocalServer {
    ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
}

#[derive(Debug)]
struct TracingInitialized;

#[test]
fn server_run_routes_cli_port_warnings_without_a_subscriber() {
    [
        (Format::Text, 0),
        (Format::Json, 0),
        (Format::Text, 1),
        (Format::Json, 1),
    ]
    .into_iter()
    .for_each(|(format, quiet)| {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("HOME", jail.directory().display().to_string());
            jail.set_env("NO_COLOR", "1");
            jail.create_file(
                "settings.toml",
                "[filesystem_snapshots]\nenabled = false\n[logging.file]\nenabled = false",
            )?;
            let manifest = local_server(LocalServer {
                custom_request_port: Some(1),
                mcp_port: Some(2),
                ..LocalServer::default()
            });
            let ports = StartupPorts {
                router_port: 9881,
                custom_request_port: 41235,
                mcp_port: 41236,
            };
            let capture = OutputCapture::new();
            let _restore_output = LogOutput::new(CliOutput::TracingDebug);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tracing::dispatcher::with_default(&tracing::Dispatch::none(), || {
                    runtime.block_on(async {
                        let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                        flags.local = true;
                        flags.disable_app_manifest_discovery = true;
                        flags.server_no_limit_change = true;
                        flags.format = Some(format);
                        let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                        discard_held_log();
                        let _ = capture.take();
                        let _pre_handler_output = LogOutput::new(CliOutput::TracingDebug);
                        ServerCommandHandler
                            .handle_with_tracing(
                                ctx,
                                ServerSubcommand::Run {
                                    args: RunArgs {
                                        config: Some("settings.toml".into()),
                                        data_dir: Some(jail.directory().join("data")),
                                        router_addr: Some("127.0.0.1".into()),
                                        ..Default::default()
                                    },
                                },
                                Verbosity::new(0, quiet),
                                |_, _| {
                                    warn_on_subdomain_port_mismatches(Some(&manifest), &ports);
                                    let (stdout, stderr) = capture.take();
                                    release_held_log();
                                    let (released_stdout, released_stderr) = capture.take();
                                    let warning = if quiet > 0 {
                                        assert!(stdout.is_empty());
                                        assert!(stderr.is_empty());
                                        assert!(released_stdout.is_empty());
                                        released_stderr
                                    } else {
                                        assert!(released_stdout.is_empty());
                                        assert!(released_stderr.is_empty());
                                        if format.is_structured() {
                                            assert!(stdout.is_empty());
                                            stderr
                                        } else {
                                            assert!(stderr.is_empty());
                                            stdout
                                        }
                                    };
                                    let warning =
                                        String::from_utf8(warning).unwrap().into_boxed_str();
                                    [
                                        "HTTP API port 41235 differs from port 1",
                                        "MCP port 41236 differs from port 2",
                                        "localServer.customRequestPort",
                                        "localServer.mcpPort",
                                    ]
                                    .into_iter()
                                    .for_each(|expected| {
                                        assert!(
                                            warning.contains(expected),
                                            "format={format}, -q={quiet}: {warning}"
                                        );
                                    });
                                    std::panic::panic_any(TracingInitialized);
                                },
                            )
                            .await
                            .unwrap();
                    })
                })
            }));
            discard_held_log();
            let panic = stopped.expect_err("stop before launching the server");
            if !panic.is::<TracingInitialized>() {
                std::panic::resume_unwind(panic);
            }
            Ok(())
        });
    });
}

#[test]
fn server_tracing_receives_loaded_settings_after_confirmed_cleanup() {
    let off = [false; 6];
    let info = [true, true, false, false, true, true];
    let debug = [true; 6];
    let targeted = [false, false, false, true, false, false];
    let rmcp = [false, false, false, false, true, true];
    [
            (None, None, 0, 0, "off", info, off),
            (Some("off"), None, 0, 0, "debug", rmcp, debug),
            (Some("debug"), None, 0, 0, "off", debug, off),
            (Some("off"), None, 3, 0, "off", debug, off),
            (Some("debug"), None, 0, 1, "debug", off, debug),
            (
                Some("logger_probe=debug"),
                None,
                0,
                0,
                "off",
                [false, false, false, true, true, true],
                off,
            ),
            (Some("off,logger_probe=debug"), None, 0, 1, "debug", targeted, debug),
            (
                Some("debug"),
                None,
                1,
                0,
                "off",
                [true, false, false, false, true, true],
                off,
            ),
            (Some("debug"), None, 0, 1, "off", off, off),
            (None, None, 0, 1, "debug", off, debug),
            (Some("off,rmcp=warn"), None, 0, 1, "off", rmcp, off),
            (
                None,
                Some("rmcp=error"),
                0,
                1,
                "off,rmcp=warn",
                [false, false, false, false, false, true],
                rmcp,
            ),
        ]
        .into_iter()
        .for_each(|(rust_log, extra, verbose, quiet, otlp_filter, expected, expected_otlp)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                let log_dir = data_dir.join("logs");
                let log_path = log_dir.join("golem.log");
                let old_state = data_dir.join("old-state");
                std::fs::create_dir_all(&log_dir).unwrap();
                std::fs::write(&old_state, b"old state").unwrap();
                std::fs::write(&log_path, b"old log").unwrap();
                jail.set_env("GOLEM__LOGGING__FILE_DIR", log_dir.display().to_string());
                if let Some(rust_log) = rust_log {
                    jail.set_env("RUST_LOG", rust_log);
                }
                if let Some(extra) = extra {
                    jail.set_env(LOG_FILTER_ENV_VAR, extra);
                }
                jail.set_env("GOLEM_OTLP_FILTER", otlp_filter);
                jail.create_file(
                    "settings.toml",
                    r#"
                [logging.stdout]
                enabled = false
                json = true
                [logging.file]
                enabled = true
                [logging]
                file_name = "golem.log"
                file_truncate = true
                [filesystem_snapshots]
                enabled = false
                [otlp]
                enabled = true
                host = "collector.startup.test"
                port = 14318
            "#,
                )?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    flags.yes = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run {
                                args: RunArgs {
                                    config: Some("settings.toml".into()),
                                    data_dir: Some(data_dir.clone()),
                                    router_addr: Some("127.0.0.1".to_string()),
                                    clean: true,
                                    ..Default::default()
                                },
                            },
                            Verbosity::new(verbose, quiet),
                            |logging, make_filter| {
                                assert!(!old_state.exists(), "cleanup precedes tracing");
                                assert!(!logging.stdout.enabled);
                                assert!(logging.stdout.json);
                                assert!(logging.otlp.enabled);
                                assert_eq!(logging.otlp.host, "collector.startup.test");
                                assert_eq!(logging.otlp.port, 14318);
                                [
                                    ("stdout", Output::Stdout, expected),
                                    ("stderr", Output::Stderr, expected),
                                    ("file", Output::File, expected),
                                    ("otlp", Output::Otlp, expected_otlp),
                                ]
                                .into_iter()
                                .for_each(|(output_name, output, expected)| {
                                    let probe_log = jail.directory().join(output_name).into_boxed_path();
                                    let writer = Arc::new(std::fs::File::create(&probe_log).unwrap());
                                    let subscriber = tracing_subscriber::registry().with(
                                        tracing_subscriber::fmt::layer()
                                            .with_ansi(false)
                                            .with_writer(writer)
                                            .with_filter(make_filter(output)),
                                    );
                                    let observed = tracing::subscriber::with_default(subscriber, || {
                                        let observed = [
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::ERROR),
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::INFO),
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::DEBUG),
                                            tracing::enabled!(target: "logger_probe", tracing::Level::DEBUG),
                                        ];
                                        tracing::warn!(target: "rmcp", "rmcp-warning-probe");
                                        tracing::error!(target: "rmcp", "rmcp-error-probe");
                                        observed
                                    });
                                    let contents = std::fs::read_to_string(&probe_log).unwrap().into_boxed_str();
                                    let observed = [
                                        observed[0], observed[1], observed[2], observed[3],
                                        contents.contains("rmcp-warning-probe"),
                                        contents.contains("rmcp-error-probe"),
                                    ];
                                    assert_eq!(
                                        observed, expected,
                                        "{output_name}: RUST_LOG={rust_log:?}, GOLEM_LOG_FILTER={extra:?}, -v={verbose}, -q={quiet}, GOLEM_OTLP_FILTER={otlp_filter}"
                                    );
                                });
                                let destination = Path::new(logging.file_dir.as_deref().unwrap_or("."))
                                    .join(logging.file_name.as_ref().unwrap());
                                assert_eq!(destination, log_path);
                                assert!(logging.file.enabled);
                                assert!(logging.file_truncate);
                                let writer = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(logging.file_truncate)
                                    .open(&destination)
                                    .unwrap();
                                let subscriber = tracing_subscriber::fmt()
                                    .with_ansi(false)
                                    .with_writer(Arc::new(writer))
                                    .finish();
                                tracing::subscriber::with_default(subscriber, || {
                                    tracing::info!("post-clean-log-event");
                                });
                                let contents = std::fs::read_to_string(&destination).unwrap();
                                assert!(contents.contains("post-clean-log-event"), "{contents}");
                                assert!(!contents.contains("old log"));
                                std::panic::panic_any(TracingInitialized);
                            },
                        )
                        .await.unwrap();
                })));
                assert!(stopped.unwrap_err().is::<TracingInitialized>());
                Ok(())
            });
        });
}

#[test]
fn server_tracing_preserves_target_filters_and_mcp_suppression() {
    let off = [false; 9];
    let mcp = [false, true, true, false, false, false, false, false, false];
    let warning = [false, false, true, false, false, false, false, false, false];
    let verbose = [true, false, true, true, true, true, true, true, true];
    let no_mcp = [true, false, false, true, true, true, true, true, true];
    [
            (None, None, 3, "off,rmcp=debug", verbose, mcp),
            (Some("rmcp=off"), None, 3, "off,rmcp=debug", no_mcp, mcp),
            (Some("rmcp=debug"), None, 0, "off", mcp, off),
            (Some("off"), Some("rmcp=debug"), 0, "off", mcp, off),
            (Some("debug"), Some("rmcp=off"), 0, "off,rmcp=debug", no_mcp, mcp),
            (Some(""), None, 0, "off", warning, off),
            (Some("rmcp=definitely_invalid"), None, 0, "off", warning, off),
            (Some("rmcp=debug,rmcp=definitely_invalid"), None, 0, "off", mcp, off),
            (
                Some("off"),
                Some(" , logger_probe=debug , rmcp=definitely_invalid , , rmcp=error , "),
                0,
                "off,rmcp=debug",
                [true, false, false, true, true, true, true, true, false],
                mcp,
            ),
            (
                Some("off,rmcp=off,[request{ratio=1.0}]=debug"),
                None,
                0,
                "off",
                [false, false, false, true, false, false, false, false, false],
                off,
            ),
            (
                Some("off,rmcp=off,[request{ratio=1}]=debug"),
                None,
                0,
                "off",
                [false, false, false, false, true, false, false, false, false],
                off,
            ),
            (
                Some("off,rmcp=off,field_presence_probe[{id}]=debug"),
                None,
                0,
                "off",
                [false, false, false, false, false, false, true, false, true],
                off,
            ),
            (
                Some("off,rmcp=off"),
                Some("field_presence_probe[{id}]=debug"),
                0,
                "off",
                [false, false, false, false, false, false, false, false, true],
                off,
            ),
            (
                Some("off,rmcp=off,field_presence_probe[{id}]=debug"),
                Some("field_presence_probe[{id}]=off"),
                0,
                "off",
                [false, false, false, false, false, false, true, false, false],
                off,
            ),
        ]
        .into_iter()
        .for_each(|(rust_log, extra, verbose, otlp, ordinary, exported)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                if let Some(value) = rust_log {
                    jail.set_env("RUST_LOG", value);
                }
                if let Some(value) = extra {
                    jail.set_env(golem_cli::LOG_FILTER_ENV_VAR, value);
                }
                jail.set_env("GOLEM_OTLP_FILTER", otlp);
                jail.create_file("settings.toml", "[filesystem_snapshots]\nenabled = false")?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async {
                        let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                        flags.local = true;
                        flags.disable_app_manifest_discovery = true;
                        flags.server_no_limit_change = true;
                        let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                        ServerCommandHandler
                            .handle_with_tracing(
                                ctx,
                                ServerSubcommand::Run {
                                    args: RunArgs {
                                        config: Some("settings.toml".into()),
                                        data_dir: Some(jail.directory().join("data")),
                                        router_addr: Some("127.0.0.1".into()),
                                        ..Default::default()
                                    },
                                },
                                Verbosity::new(verbose, 0),
                                |_, make_filter| {
                                    let layers = [
                                        ("stdout", Output::Stdout),
                                        ("stderr", Output::Stderr),
                                        ("file", Output::File),
                                        ("otlp", Output::Otlp),
                                    ]
                                    .into_iter()
                                    .map(|(name, output)| {
                                        let writer = Arc::new(
                                            std::fs::File::create(jail.directory().join(name))
                                                .unwrap(),
                                        );
                                        tracing_subscriber::fmt::layer()
                                            .without_time()
                                            .with_ansi(false)
                                            .with_writer(writer)
                                            .with_filter(make_filter(output))
                                            .boxed()
                                    })
                                    .collect::<Vec<_>>();
                                    let subscriber = tracing_subscriber::registry().with(layers);
                                    tracing::subscriber::with_default(subscriber, || {
                                        tracing::debug!(target: "logger_probe", "general-debug");
                                        tracing::debug!(target: "rmcp::service", "synthetic-mcp-body");
                                        tracing::warn!(target: "rmcp::service", "mcp-warning");
                                        {
                                            let span = tracing::info_span!("request", ratio = 1.0_f64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "float-span-event");
                                        }
                                        {
                                            let span = tracing::info_span!("request", ratio = 1_u64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "integer-span-event");
                                        }
                                        tracing::debug!(target: "logger_probe", "field-before");
                                        {
                                            let span = tracing::info_span!(target: "field_presence_probe", "request", id = 7_u64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "field-inside");
                                        }
                                        tracing::debug!(target: "logger_probe", "field-after");
                                        tracing::debug!(target: "field_presence_probe", id = 7_u64, "field-direct");
                                    });
                                    [
                                        ("stdout", ordinary),
                                        ("stderr", ordinary),
                                        ("file", ordinary),
                                        ("otlp", exported),
                                    ]
                                    .into_iter()
                                    .for_each(|(name, expected)| {
                                        let contents = std::fs::read_to_string(
                                            jail.directory().join(name),
                                        )
                                        .unwrap();
                                        [
                                            "general-debug",
                                            "synthetic-mcp-body",
                                            "mcp-warning",
                                            "float-span-event",
                                            "integer-span-event",
                                            "field-before",
                                            "field-inside",
                                            "field-after",
                                            "field-direct",
                                        ]
                                        .into_iter()
                                        .zip(expected)
                                        .for_each(|(marker, expected)| {
                                            assert_eq!(
                                                contents.contains(marker),
                                                expected,
                                                "{name}: {marker}, RUST_LOG={rust_log:?}, GOLEM_LOG_FILTER={extra:?}, -v={verbose}, GOLEM_OTLP_FILTER={otlp}\n{contents}"
                                            );
                                        });
                                    });
                                    std::panic::panic_any(TracingInitialized);
                                },
                            )
                            .await
                            .unwrap();
                    })
                }));
                let panic = stopped.expect_err("server startup must reach tracing");
                if !panic.is::<TracingInitialized>() {
                    std::panic::resume_unwind(panic);
                }
                Ok(())
            });
        });
}

#[test]
fn server_tracing_preserves_logs_when_configuration_refuses_startup() {
    [true, false].into_iter().for_each(|public_exposure| {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("HOME", jail.directory().display().to_string());
            let data_dir = jail.directory().join("data");
            std::fs::create_dir_all(&data_dir).unwrap();
            let log_path = data_dir.join("golem.log");
            let old_state = data_dir.join("old-state");
            let key_path = data_dir.join(crate::filesystem_config::KEY_FILE);
            std::fs::write(&log_path, b"preserve this log").unwrap();
            std::fs::write(&old_state, b"preserve this state").unwrap();
            std::fs::write(&key_path, b"corrupt").unwrap();
            jail.set_env("GOLEM__LOGGING__FILE_DIR", data_dir.display().to_string());
            jail.create_file(
                "settings.toml",
                &format!(
                    r#"
                    [logging]
                    file_name = "golem.log"
                    file_truncate = true
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = {}
                "#,
                    !public_exposure
                ),
            )?;
            let args = RunArgs {
                config: Some("settings.toml".into()),
                data_dir: Some(data_dir),
                router_addr: Some(
                    if public_exposure {
                        "0.0.0.0"
                    } else {
                        "127.0.0.1"
                    }
                    .into(),
                ),
                clean: public_exposure,
                ..Default::default()
            };
            load_server_config(None, &args, SnapshotRepositoryKeySource::ValidationOnly).unwrap();
            let called = std::cell::Cell::new(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = runtime.block_on(async {
                let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                flags.local = true;
                flags.disable_app_manifest_discovery = true;
                flags.server_no_limit_change = true;
                flags.yes = true;
                let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                ServerCommandHandler
                    .handle_with_tracing(
                        ctx,
                        ServerSubcommand::Run { args },
                        Verbosity::new(0, 0),
                        |logging, _| {
                            called.set(true);
                            let destination = Path::new(logging.file_dir.as_deref().unwrap_or("."))
                                .join(logging.file_name.as_ref().unwrap());
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create(true)
                                .truncate(logging.file_truncate)
                                .open(destination)
                                .unwrap();
                        },
                    )
                    .await
            });
            let message = format!("{:#}", result.unwrap_err());
            assert!(
                message.contains(if public_exposure {
                    "Refusing to listen"
                } else {
                    "repository_key must be 128 hex characters"
                }),
                "{message}"
            );
            assert!(!called.get(), "refusal precedes tracing");
            assert_eq!(std::fs::read(&log_path).unwrap(), b"preserve this log");
            assert_eq!(std::fs::read(&old_state).unwrap(), b"preserve this state");
            assert_eq!(std::fs::read(&key_path).unwrap(), b"corrupt");
            Ok(())
        });
    });
}

#[test]
fn server_tracing_does_not_overwrite_the_retained_repository_key() {
    [
            "direct",
            "hard-link",
            "missing-parent",
            "not-created",
            "noncolliding",
            #[cfg(unix)]
            "symlink",
        ]
        .into_iter()
        .flat_map(|alias| {
            [(false, false), (true, false), (true, true)]
                .into_iter()
                .map(move |(enabled, explicit_key)| (alias, enabled, explicit_key))
        })
        .filter(|(alias, enabled, explicit_key)| *alias != "not-created" || !enabled || *explicit_key)
        .for_each(|(alias, enabled, explicit_key)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                std::fs::create_dir_all(&data_dir).unwrap();
                let key_path = data_dir.join(crate::filesystem_config::KEY_FILE);
                let key = "2a".repeat(64);
                if alias != "not-created" {
                    std::fs::write(&key_path, &key).unwrap();
                }
                let log_path = match alias {
                    "direct" | "not-created" => key_path.clone(),
                    "missing-parent" => data_dir
                        .join("not-yet")
                        .join("..")
                        .join(crate::filesystem_config::KEY_FILE),
                    _ => data_dir.join("server.log"),
                };
                match alias {
                    "hard-link" => std::fs::hard_link(&key_path, &log_path).unwrap(),
                    #[cfg(unix)]
                    "symlink" => std::os::unix::fs::symlink(&key_path, &log_path).unwrap(),
                    _ => {}
                }
                jail.set_env("GOLEM__LOGGING__FILE_NAME", log_path.display().to_string());
                jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", enabled);
                if explicit_key {
                    jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__REPOSITORY_KEY", "3b".repeat(64));
                }
                jail.create_file(
                    "settings.toml",
                    r#"
                    [logging]
                    file_truncate = true
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = true
                "#,
                )?;
                let called = std::cell::Cell::new(false);
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run {
                                args: RunArgs {
                                    config: Some("settings.toml".into()),
                                    data_dir: Some(data_dir),
                                    router_addr: Some("127.0.0.1".into()),
                                    ..Default::default()
                                },
                            },
                            Verbosity::new(0, 0),
                            |_, _| {
                                called.set(true);
                                std::fs::write(&log_path, b"log would replace key").unwrap();
                                std::panic::panic_any(TracingInitialized);
                            },
                        )
                        .await
                })));
                if alias == "noncolliding" {
                    assert!(result.unwrap_err().is::<TracingInitialized>());
                    assert!(called.get());
                    assert_eq!(std::fs::read(&log_path).unwrap(), b"log would replace key");
                } else {
                    let message = format!("{:#}", result.unwrap().unwrap_err());
                    assert!(message.contains("retained repository key"), "{message}");
                    assert!(
                        !called.get(),
                        "{alias}, enabled={enabled}, explicit_key={explicit_key}: rejection precedes tracing"
                    );
                }
                if alias == "not-created" && (!enabled || explicit_key) {
                    assert!(!key_path.exists());
                } else {
                    assert_eq!(std::fs::read_to_string(&key_path).unwrap(), key);
                }
                Ok(())
            });
        });
}

#[test]
fn server_tracing_does_not_overwrite_selected_sqlite_databases() {
    [
        ("registry_service.db", Some("REGISTRY_SERVICE__DB")),
        ("registry.db", None),
        ("shard_manager.db", None),
        ("gateway-sessions.db", None),
        ("scheduler.db", None),
        ("kv-store/kv-running_workers", None),
        ("kv-store/kv-schedule.db", None),
        ("kv-store/kv-user-defined.db", None),
        ("kv-store/epochs.db", None),
        ("custom.db", Some("REGISTRY_SERVICE__DB")),
        ("custom.db", Some("SHARD_MANAGER__PERSISTENCE")),
        ("custom.db", Some("WORKER_SERVICE__GATEWAY_SESSION_STORAGE")),
        ("custom.db", Some("WORKER_SERVICE__MCP__SESSION_STORAGE")),
        ("custom.db", Some("WORKER_EXECUTOR__SCHEDULER_STORAGE")),
        ("custom.db", Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE")),
        ("custom.db", Some("WORKER_EXECUTOR__INDEXED_STORAGE")),
        (
            "custom-indexed.db",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE"),
        ),
        (
            "other-kv/kv-running_workers",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE"),
        ),
        (
            "other-index/epochs.db",
            Some("WORKER_EXECUTOR__INDEXED_STORAGE"),
        ),
        (
            "custom.db",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__CACHE"),
        ),
        (
            "custom.db",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__PERSISTENT"),
        ),
        ("custom.db", Some("REGISTRY_SERVICE__BLOB_STORAGE")),
        ("custom.db", Some("WORKER_SERVICE__BLOB_STORAGE")),
        ("custom.db", Some("WORKER_EXECUTOR__BLOB_STORAGE")),
        (
            "custom.db",
            Some("COMPONENT_COMPILATION_SERVICE__BLOB_STORAGE"),
        ),
    ]
    .into_iter()
    .map(|(database, store)| (database, store, true))
    .chain([
        ("kv-store/server.db", None, false),
        ("kv-store/kv-worker-unrelated.db", None, false),
        ("kv-store/durable-oplog-unrelated.db", None, true),
        ("kv-store/durable-oplog-.db", None, true),
        ("kv-store/ephemeral-compressed-oplog-l2-not-uuid.db", None, true),
        ("kv-store/durable-blob-oplog-l0-unrelated.db", None, true),
        ("kv-store/durable-compressed-oplog-l02-unrelated.db", None, false),
        ("kv-store/durable-compressed-oplog-lx-unrelated.db", None, false),
        (
            "other-index/durable-oplog-unrelated.db",
            Some("WORKER_EXECUTOR__INDEXED_STORAGE"),
            true,
        ),
        (
            "other-kv/epochs.db",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE"),
            false,
        ),
        (
            "other-index/kv-schedule.db",
            Some("WORKER_EXECUTOR__INDEXED_STORAGE"),
            false,
        ),
        (
            "other-cache/kv-schedule.db",
            Some("WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__CACHE"),
            false,
        ),
        (
            "kv-store/kv-worker-1ac92b47-0d81-4c31-936a-0ea6104cdd59_2bb90c48-1de4-4e54-b638-44ed713f5420.db",
            None,
            true,
        ),
        (
            "kv-store/durable-oplog-1ac92b47-0d81-4c31-936a-0ea6104cdd59_2bb90c48-1de4-4e54-b638-44ed713f5420.db",
            None,
            true,
        ),
        (
            "kv-store/ephemeral-compressed-oplog-l2-1ac92b47-0d81-4c31-936a-0ea6104cdd59_2bb90c48-1de4-4e54-b638-44ed713f5420.db",
            None,
            true,
        ),
        (
            "kv-store/durable-blob-oplog-l3-1ac92b47-0d81-4c31-936a-0ea6104cdd59_2bb90c48-1de4-4e54-b638-44ed713f5420.db",
            None,
            true,
        ),
    ])
    .flat_map(|database| {
        let recovery_files = database.2
            && (matches!(
                database.0,
                "registry_service.db" | "kv-store/kv-running_workers" | "kv-store/epochs.db"
            ) || database.0.ends_with("durable-oplog-unrelated.db")
                || database.1 == Some("WORKER_SERVICE__MCP__SESSION_STORAGE")
                || database.0 == "other-kv/kv-running_workers"
                || (database.0 == "custom.db" && database.1 == Some("REGISTRY_SERVICE__DB")));
        ["", "-wal", "-shm", "-journal"]
            .into_iter()
            .filter(move |suffix| suffix.is_empty() || recovery_files)
            .map(move |suffix| (database, suffix))
    })
    .flat_map(|database| {
        [
            "direct",
            "not-created",
            "hard-link",
            "noncolliding",
            "uri",
            "uri-relative",
            "uri-authority",
            "uri-absent",
            "uri-nul",
            "uri-memory",
            "uri-memory-then-disk",
            "uri-disk-then-memory",
            "literal-percent",
            #[cfg(unix)]
            "multi-uri",
            #[cfg(unix)]
            "multi-uri-absent",
            #[cfg(unix)]
            "multi-uri-hard-link",
            #[cfg(unix)]
            "multi-uri-query",
            #[cfg(unix)]
            "multi-uri-fragment",
            #[cfg(unix)]
            "multi-uri-nul",
            #[cfg(unix)]
            "multi-uri-indexed",
            #[cfg(unix)]
            "multi-uri-indexed-absent",
            #[cfg(windows)]
            "windows-case",
            #[cfg(windows)]
            "windows-database-symlink",
            #[cfg(unix)]
            "symlink",
            #[cfg(unix)]
            "database-symlink",
            #[cfg(unix)]
            "dangling-database-symlink",
            #[cfg(unix)]
            "chained-database-symlink",
        ]
        .into_iter()
        .filter(move |alias| {
            (!matches!(*alias, "dangling-database-symlink" | "chained-database-symlink")
                || !database.1.is_empty())
                && (!(alias.starts_with("uri") || *alias == "literal-percent")
                    || (database.0.0 == "custom.db" && database.0.1 == Some("REGISTRY_SERVICE__DB")))
                && (!alias.starts_with("multi-uri") || database.0.0.starts_with("other-"))
                && (!alias.starts_with("multi-uri-indexed") || database.0.1 == Some("WORKER_EXECUTOR__INDEXED_STORAGE"))
                && (!alias.starts_with("windows-")
                    || (database.0.0 == "custom.db" && database.0.1 == Some("REGISTRY_SERVICE__DB")))
                && (*alias != "windows-database-symlink" || !database.1.is_empty())
        })
        .flat_map(move |alias| [false, true].map(|retained_key| (database, alias, retained_key)))
    })
    .for_each(|(((database, selected_store, selected), suffix), alias, retained_key)| {
        figment::Jail::expect_with(|jail| {
            let selected = selected && !matches!(alias, "uri-memory" | "uri-disk-then-memory");
            let selected = if alias.starts_with("multi-uri") && !alias.starts_with("multi-uri-indexed") {
                selected_store.is_some_and(|store| store.contains("KEY_VALUE_STORAGE"))
                    && (selected || matches!(alias, "multi-uri-query" | "multi-uri-fragment" | "multi-uri-nul"))
            } else {
                selected
            };
            jail.clear_env();
            jail.set_env("HOME", jail.directory().display().to_string());
            let data_dir = jail.directory().join("data");
            std::fs::create_dir_all(&data_dir).unwrap();
            let mut database_path = if selected_store.is_some() {
                jail.directory().join("external").join(database)
            } else {
                data_dir.join(database)
            };
            if alias == "literal-percent" {
                database_path.set_file_name("custom%20name.db");
            }
            if alias == "windows-case" {
                database_path.set_file_name("cüstom.db");
            }
            let resolved_database = if cfg!(unix) && alias.ends_with("database-symlink") {
                database_path.with_file_name("resolved.db")
            } else if alias.starts_with("multi-uri-indexed") {
                jail.directory()
                    .join(format!("file:{}", database_path.parent().unwrap().display()))
                    .join(database_path.file_name().unwrap())
            } else {
                database_path.clone()
            };
            std::fs::create_dir_all(resolved_database.parent().unwrap()).unwrap();
            let state_path = resolved_database.with_file_name(format!(
                "{}{suffix}",
                resolved_database.file_name().unwrap().to_str().unwrap()
            ));
            if !matches!(alias, "not-created" | "uri-absent" | "windows-case" | "multi-uri-absent" | "multi-uri-indexed-absent") {
                std::fs::write(&state_path, b"preserve selected SQLite state").unwrap();
            }
            #[cfg(windows)]
            if alias == "windows-database-symlink" {
                let target = database_path.with_file_name("resolved.db");
                std::fs::write(&target, b"selected database").unwrap();
                std::os::windows::fs::symlink_file(target, &database_path).unwrap();
            }
            #[cfg(unix)]
            if alias.ends_with("database-symlink") {
                if alias == "database-symlink" && !suffix.is_empty() {
                    std::fs::write(&resolved_database, b"selected database").unwrap();
                }
                let target = if alias == "chained-database-symlink" {
                    let intermediate = database_path.with_file_name("intermediate.db");
                    std::os::unix::fs::symlink(
                        resolved_database.file_name().unwrap(),
                        &intermediate,
                    )
                    .unwrap();
                    intermediate
                } else {
                    resolved_database.clone()
                };
                std::os::unix::fs::symlink(
                    if alias == "database-symlink" {
                        target.as_os_str()
                    } else {
                        target.file_name().unwrap()
                    },
                    &database_path,
                )
                .unwrap();
            }
            let key_path = data_dir.join(crate::filesystem_config::KEY_FILE);
            if retained_key {
                std::fs::write(&key_path, "2a".repeat(64)).unwrap();
            }
            if let Some(store) = selected_store {
                if store.contains("__CONFIG__") {
                    jail.set_env(
                        "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE",
                        "NamespaceRouted",
                    );
                    let other = if store.ends_with("__CACHE") {
                        "PERSISTENT"
                    } else {
                        "CACHE"
                    };
                    let prefix = format!(
                        "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__{other}"
                    );
                    jail.set_env(format!("{prefix}__TYPE"), "Sqlite");
                    jail.set_env(
                        format!("{prefix}__CONFIG__DATABASE"),
                        data_dir.join("unused.db").display().to_string(),
                    );
                    jail.set_env(format!("{prefix}__CONFIG__MAX_CONNECTIONS"), 4);
                    jail.set_env(format!("{prefix}__CONFIG__FOREIGN_KEYS"), false);
                }
                jail.set_env(
                    format!("GOLEM_ADVANCED__{store}__CONFIG__MAX_CONNECTIONS"),
                    4,
                );
                jail.set_env(
                    format!("GOLEM_ADVANCED__{store}__CONFIG__FOREIGN_KEYS"),
                    false,
                );
                if database.starts_with("other-") {
                    jail.set_env(format!("GOLEM_ADVANCED__{store}__TYPE"), "MultiSqlite");
                    let root = match alias {
                        "multi-uri" | "multi-uri-absent" | "multi-uri-hard-link" | "multi-uri-indexed" | "multi-uri-indexed-absent" => format!("file:{}", database_path.parent().unwrap().display()),
                        "multi-uri-query" => format!("file:{}?unused=value", database_path.display()),
                        "multi-uri-fragment" => format!("file:{}#ignored", database_path.display()),
                        "multi-uri-nul" => format!("file:{}%00ignored", database_path.display()),
                        _ => database_path.parent().unwrap().display().to_string(),
                    };
                    jail.set_env(
                        format!("GOLEM_ADVANCED__{store}__CONFIG__ROOT_DIR"),
                        root,
                    );
                    if store == "WORKER_EXECUTOR__KEY_VALUE_STORAGE" {
                        jail.set_env(
                            "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__TYPE",
                            "InMemory",
                        );
                    }
                } else {
                    jail.set_env(format!("GOLEM_ADVANCED__{store}__TYPE"), "Sqlite");
                    if store == "WORKER_SERVICE__MCP__SESSION_STORAGE" {
                        jail.set_env(
                            format!("GOLEM_ADVANCED__{store}__CONFIG__CLEANUP_INTERVAL"),
                            "5m",
                        );
                    }
                    let selected_database = if database == "custom-indexed.db" {
                        jail.set_env(
                            "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__TYPE",
                            "KVStoreSqlite",
                        );
                        database_path.with_file_name("custom.db")
                    } else {
                        database_path.clone()
                    };
                    let selected_database = selected_database.display().to_string().replace('\\', "/");
                    let selected_database = match alias {
                        "uri" | "uri-absent" => format!("file:{}?mode=rwc&unknown=a+b#ignored", selected_database.replace("custom.db", "%63ustom.db")),
                        "uri-relative" => "file:external/%63ustom.db?mode=rwc#ignored".to_string(),
                        "uri-authority" => format!("file://localhost/{}?mode=rwc", selected_database.trim_start_matches('/')),
                        "uri-nul" => format!("file:{selected_database}%00ignored?mode=rwc"),
                        "uri-memory" => format!("file:{selected_database}?%6dode=memory"),
                        "uri-memory-then-disk" => format!("file:{selected_database}?mode=memory&mode=rwc"),
                        "uri-disk-then-memory" => format!("file:{selected_database}?mode=rwc&mode=memory"),
                        _ => selected_database,
                    };
                    jail.set_env(
                        format!("GOLEM_ADVANCED__{store}__CONFIG__DATABASE"),
                        selected_database,
                    );
                }
            }
            let log_path = if alias == "windows-case" {
                state_path.with_file_name(state_path.file_name().unwrap().to_str().unwrap().to_uppercase())
            } else if matches!(alias, "direct" | "not-created" | "literal-percent")
                || alias.starts_with("uri") || (alias.starts_with("multi-uri") && alias != "multi-uri-hard-link") || alias.ends_with("database-symlink") {
                state_path.clone()
            } else {
                data_dir.join("server.log")
            };
            match alias {
                "hard-link" | "multi-uri-hard-link" => std::fs::hard_link(&state_path, &log_path).unwrap(),
                #[cfg(unix)]
                "symlink" => std::os::unix::fs::symlink(&state_path, &log_path).unwrap(),
                _ => {}
            }
            jail.set_env("GOLEM__LOGGING__FILE_NAME", log_path.display().to_string());
            jail.create_file(
                "settings.toml",
                "[logging.file]\nenabled = true\n[filesystem_snapshots]\nenabled = false",
            )?;
            let called = std::cell::Cell::new(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run {
                                args: RunArgs {
                                    config: Some("settings.toml".into()),
                                    data_dir: Some(data_dir),
                                    router_addr: Some("127.0.0.1".into()),
                                    ..Default::default()
                                },
                            },
                            Verbosity::new(0, 0),
                            |logging, _| {
                                called.set(true);
                                assert!(logging.file_truncate);
                                std::fs::write(&log_path, b"log would replace state").unwrap();
                                std::panic::panic_any(TracingInitialized);
                            },
                        )
                        .await
                })
            }));
            if !selected || alias == "noncolliding" {
                assert!(result.unwrap_err().is::<TracingInitialized>());
                assert!(called.get());
            } else {
                let refusal = result.unwrap_or_else(|_| {
                    panic!("tracing opened {database}{suffix}, {alias}, retained_key={retained_key}")
                });
                let message = format!("{:#}", refusal.unwrap_err());
                assert!(message.contains("selected SQLite database"), "{message}");
                assert!(
                    !called.get(),
                    "{database}, {alias}, retained_key={retained_key}"
                );
            }
            if matches!(alias, "not-created" | "uri-absent" | "windows-case" | "multi-uri-absent" | "multi-uri-indexed-absent") && selected {
                assert!(!state_path.exists());
            } else {
                assert_eq!(
                    std::fs::read(&state_path).unwrap(),
                    if !selected && alias != "noncolliding" {
                        b"log would replace state".as_slice()
                    } else {
                        b"preserve selected SQLite state".as_slice()
                    }
                );
            }
            assert_eq!(key_path.exists(), retained_key);
            Ok(())
        });
    });
}

#[cfg(target_os = "linux")]
#[test]
#[test_r::timeout("30s")]
async fn server_tracing_does_not_read_open_noncolliding_sinks() {
    use rustix::fs::{CWD, Mode, mkfifoat};
    use std::os::unix::fs::PermissionsExt;

    tokio::task::spawn_blocking(|| {
        [false, true].into_iter().for_each(|fifo| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                std::fs::create_dir_all(&data_dir).unwrap();
                let log_path = data_dir.join("server.log");
                if fifo {
                    mkfifoat(CWD, &log_path, Mode::RUSR | Mode::WUSR).unwrap();
                } else {
                    std::fs::write(&log_path, b"write-only log").unwrap();
                    std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o200))
                        .unwrap();
                }
                jail.set_env("GOLEM__LOGGING__FILE_NAME", log_path.display().to_string());
                jail.create_file(
                    "settings.toml",
                    r#"
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = true
                "#,
                )?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async {
                        let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                        flags.local = true;
                        flags.disable_app_manifest_discovery = true;
                        flags.server_no_limit_change = true;
                        let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                        ServerCommandHandler
                            .handle_with_tracing(
                                ctx,
                                ServerSubcommand::Run {
                                    args: RunArgs {
                                        config: Some("settings.toml".into()),
                                        data_dir: Some(data_dir.clone()),
                                        router_addr: Some("127.0.0.1".into()),
                                        ..Default::default()
                                    },
                                },
                                Verbosity::new(0, 0),
                                |_, _| {
                                    assert_eq!(
                                        std::fs::read_to_string(
                                            data_dir.join(crate::filesystem_config::KEY_FILE),
                                        )
                                        .unwrap()
                                        .len(),
                                        128,
                                    );
                                    std::panic::panic_any(TracingInitialized);
                                },
                            )
                            .await
                            .unwrap();
                    })
                }));
                assert!(stopped.unwrap_err().is::<TracingInitialized>());
                Ok(())
            });
        });
    })
    .await
    .unwrap();
}

#[test]
async fn server_shutdown_waits_for_tracked_cleanup() {
    let shutdown = Shutdown::new();
    let cleanup_finished = Arc::new(AtomicBool::new(false));
    let cleanup_finished_clone = cleanup_finished.clone();
    let shutdown_token = shutdown.token();
    shutdown.spawn(async move {
        shutdown_token.cancelled().await;
        tokio::task::yield_now().await;
        cleanup_finished_clone.store(true, Ordering::Release);
    });

    shutdown_golem_services(&mut JoinSet::new(), &shutdown).await;

    assert!(cleanup_finished.load(Ordering::Acquire));
}

#[test]
fn subdomain_ports_match_when_bound_ports_equal_manifest_or_default_ports() {
    let manifest = local_server(LocalServer {
        custom_request_port: Some(9008),
        ..LocalServer::default()
    });
    let ports = StartupPorts {
        router_port: 9881,
        custom_request_port: 9008,
        mcp_port: DEFAULT_LOCAL_MCP_PORT,
    };

    assert_eq!(subdomain_port_mismatches(Some(&manifest), &ports), vec![]);
}

#[test]
fn subdomain_ports_use_defaults_without_local_server_section() {
    // e.g. `--custom-request-port 0` in an app whose manifest has no `localServer`
    let ports = StartupPorts {
        router_port: 9881,
        custom_request_port: 41235,
        mcp_port: DEFAULT_LOCAL_MCP_PORT,
    };

    assert_eq!(
        subdomain_port_mismatches(None, &ports),
        vec![SubdomainPortMismatch {
            deployment_kind: "HTTP API",
            manifest_field: "localServer.customRequestPort",
            flag: "--custom-request-port",
            expanded_port: DEFAULT_LOCAL_CUSTOM_REQUEST_PORT,
            bound_port: 41235,
        }]
    );
}

#[test]
fn subdomain_ports_mismatch_when_bound_ports_differ() {
    let manifest = local_server(LocalServer {
        custom_request_port: Some(9008),
        ..LocalServer::default()
    });
    // e.g. `--custom-request-port 0 --mcp-port 0`, OS-assigned ports
    let ports = StartupPorts {
        router_port: 9881,
        custom_request_port: 41235,
        mcp_port: 41236,
    };

    assert_eq!(
        subdomain_port_mismatches(Some(&manifest), &ports),
        vec![
            SubdomainPortMismatch {
                deployment_kind: "HTTP API",
                manifest_field: "localServer.customRequestPort",
                flag: "--custom-request-port",
                expanded_port: 9008,
                bound_port: 41235,
            },
            SubdomainPortMismatch {
                deployment_kind: "MCP",
                manifest_field: "localServer.mcpPort",
                flag: "--mcp-port",
                expanded_port: DEFAULT_LOCAL_MCP_PORT,
                bound_port: 41236,
            },
        ]
    );
}

#[test]
fn clean_rejects_filesystem_root() {
    let current_dir = std::env::current_dir().unwrap();
    let root = current_dir.ancestors().last().unwrap();

    let error = resolve_clean_data_dir(root).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Refusing to clean filesystem root")
    );
}

#[test]
fn clean_resolves_relative_data_dir() {
    let data_dir = resolve_clean_data_dir(Path::new("local-server-data")).unwrap();

    assert!(data_dir.is_absolute());
    assert!(data_dir.ends_with(Path::new("local-server-data")));

    let absolute_data_dir = std::env::current_dir().unwrap().join("local-server-data");
    assert_eq!(
        resolve_clean_data_dir(&absolute_data_dir).unwrap(),
        absolute_data_dir
    );
}

#[cfg(unix)]
mod unix {
    use super::*;
    use test_r::test;

    #[test]
    fn clean_resolves_intermediate_symlink_without_following_final_symlink() {
        use std::os::unix::fs::symlink;

        let test_root =
            std::env::temp_dir().join(format!("golem-clean-symlink-test-{}", std::process::id()));
        let actual_parent = test_root.join("actual");
        let intermediate_link = test_root.join("intermediate-link");
        let final_link = actual_parent.join("final-link");
        std::fs::create_dir_all(&actual_parent).unwrap();
        symlink(&actual_parent, &intermediate_link).unwrap();
        symlink(actual_parent.join("final-target"), &final_link).unwrap();

        let resolved_intermediate =
            resolve_clean_data_dir(&intermediate_link.join("data")).unwrap();
        let resolved_final = resolve_clean_data_dir(&final_link).unwrap();
        let canonical_parent = std::fs::canonicalize(&actual_parent).unwrap();

        assert_eq!(resolved_intermediate, canonical_parent.join("data"));
        assert_eq!(resolved_final, canonical_parent.join("final-link"));

        std::fs::remove_dir_all(&test_root).unwrap();
    }
}
