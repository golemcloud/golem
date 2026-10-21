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

use super::{
    DurableStreamConfig, FilesystemSnapshotStoreConfig, FilesystemSnapshotsConfig,
    FilesystemSnapshotsKind, FilesystemStorageMode, GolemConfig, InvocationResultsConfig, Limits,
    RetryConfig, default_filesystem_snapshot_storage_retry,
};
use golem_common::SafeDisplay;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use test_r::test;

#[test]
fn the_storage_mode_shows_its_name_and_its_root() {
    let root = || Box::from(std::path::Path::new("/var/lib/golem/agents"));
    assert_eq!(
        [
            super::FilesystemStorageMode::Temporary,
            super::FilesystemStorageMode::Directory { root: root() },
            super::FilesystemStorageMode::ManagedXfs {
                root: root(),
                xfs_quotad_socket: Box::from(std::path::Path::new(
                    "/run/golem-quota/xfs-quotad.sock"
                )),
            },
            super::FilesystemStorageMode::ReflinkXfs { root: root() },
            super::FilesystemStorageMode::Apfs { root: root() },
        ]
        .map(|storage| storage.to_safe_string()),
        [
            "temporary directories",
            "directory at /var/lib/golem/agents",
            "managed XFS at /var/lib/golem/agents, with xfs-quotad at \
             /run/golem-quota/xfs-quotad.sock",
            "reflink XFS at /var/lib/golem/agents",
            "APFS for local development at /var/lib/golem/agents",
        ]
    );
}

#[test]
fn default_listeners_bind_ipv4_and_registration_uses_the_source_ip() {
    let config = GolemConfig::default();

    assert_eq!(config.http_addr().to_string(), "0.0.0.0:8082");
    assert_eq!(config.grpc_addr().to_string(), "0.0.0.0:9093");
    assert_eq!(config.grpc.advertised_ip, None);
}

#[test]
fn bind_address_and_advertised_ip_are_read_from_toml() {
    use figment::Figment;
    use figment::providers::{Format, Serialized, Toml};

    let config: GolemConfig = Figment::new()
        .merge(Serialized::defaults(GolemConfig::default()))
        .merge(Toml::string(
            r#"
                bind_address = "::"

                [grpc]
                advertised_ip = "fdaa:0:1:a7b::2"
                "#,
        ))
        .extract()
        .unwrap();

    assert_eq!(config.grpc_addr().to_string(), "[::]:9093");
    assert_eq!(
        config.grpc.advertised_ip,
        Some("fdaa:0:1:a7b::2".parse().unwrap())
    );
    config.validate().unwrap();
}

#[test]
fn config_rejects_an_advertised_ip_that_cannot_be_dialed() {
    for advertised_ip in [
        "0.0.0.0",
        "::",
        "::ffff:0.0.0.0",
        "255.255.255.255",
        "224.0.0.1",
        "ff02::1",
        "fe80::1",
    ] {
        let mut config = GolemConfig {
            bind_address: "::".parse().unwrap(),
            ..Default::default()
        };
        config.grpc.advertised_ip = Some(advertised_ip.parse().unwrap());

        assert!(
            config.validate().is_err(),
            "{advertised_ip} must be rejected"
        );
    }
}

#[test]
fn config_rejects_an_ipv6_advertised_ip_on_an_ipv4_listener() {
    let mut config = GolemConfig::default();
    config.grpc.advertised_ip = Some("fdaa:0:1:a7b::2".parse().unwrap());
    assert!(config.validate().is_err());

    config.bind_address = "::".parse().unwrap();
    config.validate().unwrap();

    let mut config = GolemConfig::default();
    config.grpc.advertised_ip = Some("::ffff:10.0.0.5".parse().unwrap());
    config.validate().unwrap();

    let mut config = GolemConfig {
        bind_address: "::ffff:0.0.0.0".parse().unwrap(),
        ..Default::default()
    };
    config.grpc.advertised_ip = Some("fdaa:0:1:a7b::2".parse().unwrap());
    assert!(config.validate().is_err());
}

#[test]
fn config_rejects_an_ipv4_advertised_ip_on_a_specific_ipv6_listener() {
    let mut config = GolemConfig {
        bind_address: "fdaa:0:1:a7b::2".parse().unwrap(),
        ..Default::default()
    };
    config.grpc.advertised_ip = Some("10.0.0.5".parse().unwrap());
    assert!(config.validate().is_err());

    config.bind_address = "::".parse().unwrap();
    config.validate().unwrap();
}

#[test]
fn config_accepts_separate_s3_snapshot_buckets() {
    let config = GolemConfig {
        blob_storage: golem_service_base::config::BlobStorageConfig::default_s3(),
        ..Default::default()
    };

    config.validate().unwrap();
}

#[test]
fn default_and_example_configs_are_valid() {
    use golem_common::config::HasConfigExamples;

    GolemConfig::default().validate().unwrap();
    for (name, config) in GolemConfig::examples() {
        config
            .validate()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn config_accepts_one_s3_bucket_for_every_namespace_with_namespace_prefixes() {
    let bucket = "golem".to_string();
    let config = GolemConfig {
        blob_storage: golem_service_base::config::BlobStorageConfig::S3(
            golem_service_base::config::S3BlobStorageConfig {
                namespace_prefixes: true,
                compilation_cache_bucket: bucket.clone(),
                custom_data_bucket: bucket.clone(),
                oplog_payload_bucket: bucket.clone(),
                compressed_oplog_buckets: vec![bucket.clone()],
                initial_agent_files_bucket: bucket.clone(),
                components_bucket: bucket.clone(),
                filesystem_snapshots_bucket: bucket,
                ..Default::default()
            },
        ),
        ..Default::default()
    };

    config.validate().unwrap();
}

#[test]
fn config_rejects_shared_s3_snapshot_bucket() {
    let config = GolemConfig {
        blob_storage: golem_service_base::config::BlobStorageConfig::S3(
            golem_service_base::config::S3BlobStorageConfig {
                custom_data_bucket: "shared-bucket".to_string(),
                filesystem_snapshots_bucket: "shared-bucket".to_string(),
                ..Default::default()
            },
        ),
        ..Default::default()
    };
    assert!(matches!(
        config.filesystem_snapshots,
        FilesystemSnapshotsConfig::Disabled(_)
    ));

    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("filesystem_snapshots_bucket"), "{error}");
    assert!(error.contains("custom_data_bucket"), "{error}");
}

#[test]
fn mcp_transport_config_roundtrips_and_validates() {
    let mut config = GolemConfig::default();
    config.mcp_transport.request_bytes = 1234;
    config.mcp_transport.concurrency = 3;
    let serialized = serde_json::to_value(&config).unwrap();
    let decoded: GolemConfig = serde_json::from_value(serialized).unwrap();
    assert_eq!(decoded.mcp_transport.request_bytes, 1234);
    assert_eq!(decoded.mcp_transport.concurrency, 3);
    assert!(decoded.validate().is_ok());

    config.mcp_transport.concurrency = 0;
    assert!(config.validate().is_err());
}

#[test]
fn durable_stream_config_uses_byte_size() {
    let config = DurableStreamConfig::default();
    let mut serialized = serde_json::to_value(&config).unwrap();
    assert_eq!(serialized["external_batch_max_size"], "8388608 B");
    assert!(serialized.get("external_batch_max_bytes").is_none());
    serialized["external_batch_max_size"] = Value::from("1536 KiB");
    let decoded: DurableStreamConfig = serde_json::from_value(serialized.clone()).unwrap();
    assert_eq!(decoded.external_batch_max_size, 1_572_864);
    assert!(decoded.validate().is_ok());
    assert!(
        decoded
            .to_safe_string()
            .contains("external batch maximum size: 1.50 MiB")
    );
    for invalid in ["0 B", "1.5 MiB"] {
        serialized["external_batch_max_size"] = Value::from(invalid);
        assert!(serde_json::from_value::<DurableStreamConfig>(serialized.clone()).is_err());
    }
}

#[test]
fn durable_stream_config_requires_nonzero_recovery_retry_without_a_lease_deadline() {
    let mut config = DurableStreamConfig::default();
    assert!(config.validate().is_ok());
    config.reconciliation_interval = Duration::from_secs(3600);
    assert!(config.validate().is_ok());
    config.reconciliation_interval = Duration::ZERO;
    assert!(config.validate().is_err());
}

#[test]
fn invocation_results_config_rejects_invalid_lookup_structures() {
    let mut config = InvocationResultsConfig::default();
    assert!(config.validate().is_ok());

    config.bloom_bits = 0;
    assert!(config.validate().is_err());

    config.bloom_bits = 1;
    config.bloom_hashes = 0;
    assert!(config.validate().is_err());

    config.bloom_hashes = 1;
    config.physical_index_catch_up_chunk_size = 0;
    assert!(config.validate().is_err());
}

#[test]
fn mcp_oauth_deadline_leaves_time_for_registry_response() {
    let timeout = GolemConfig::default()
        .registry_service
        .client_config
        .request_timeout
        .unwrap();
    assert!(golem_mcp_import::oauth::Limits::default().timeout + Duration::from_secs(5) < timeout);
}

#[test]
fn invocation_results_config_constructs_configured_membership() {
    let config = InvocationResultsConfig {
        recent_capacity: 2,
        bloom_bits: 128,
        bloom_hashes: 3,
        ..InvocationResultsConfig::default()
    };
    let mut membership = config.membership();
    for index in 1..=6 {
        membership.insert(
            golem_common::model::IdempotencyKey::fresh(),
            golem_common::model::oplog::OplogIndex::from_u64(index),
        );
    }

    assert_eq!(membership.len(), 2);
    assert!(!membership.is_exact_complete());
}

#[test]
fn live_stream_event_broadcast_capacity_defaults_to_32() {
    let limits = Limits::default();

    assert_eq!(limits.live_stream_event_broadcast_capacity.get(), 32);
    let decoded: Limits = serde_json::from_value(serde_json::to_value(&limits).unwrap()).unwrap();
    assert_eq!(decoded.live_stream_event_broadcast_capacity.get(), 32);
    assert!(
        limits
            .to_safe_string()
            .contains("live stream event broadcast capacity: 32")
    );
}

#[test]
fn live_stream_event_broadcast_capacity_rejects_zero() {
    let mut serialized = serde_json::to_value(Limits::default()).unwrap();
    let Value::Object(fields) = &mut serialized else {
        panic!("limits must serialize as an object");
    };
    fields.insert(
        "live_stream_event_broadcast_capacity".to_string(),
        Value::from(0),
    );

    assert!(serde_json::from_value::<Limits>(serialized).is_err());
}

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\
                   202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

fn enabled(config: Value) -> Result<FilesystemSnapshotStoreConfig, String> {
    serde_json::from_value::<FilesystemSnapshotsConfig>(json!({
        "type": "Enabled",
        "config": config,
    }))
    .map_err(|error| error.to_string())
    .and_then(|parsed| match parsed {
        FilesystemSnapshotsConfig::Enabled(store) => Ok(*store),
        FilesystemSnapshotsConfig::Disabled(_) => Err("disabled".to_string()),
    })
}

fn refusal(config: Value) -> String {
    enabled(config).unwrap_err()
}

#[test]
fn filesystem_snapshots_are_disabled_by_default() {
    let config = GolemConfig::default().filesystem_snapshots;

    assert!(
        matches!(config, FilesystemSnapshotsConfig::Disabled(_)),
        "{config:?}"
    );
}

#[test]
fn filesystem_snapshots_serialization_uses_enabled_and_disabled() {
    [
        (FilesystemSnapshotsKind::Enabled, "Enabled"),
        (FilesystemSnapshotsKind::Disabled, "Disabled"),
    ]
    .into_iter()
    .for_each(|(kind, name)| {
        let encoded = serde_json::to_value(kind).unwrap();
        assert_eq!(encoded, json!(name));
        assert_eq!(
            serde_json::from_value::<FilesystemSnapshotsKind>(encoded).unwrap(),
            kind,
        );
    });
    let config: FilesystemSnapshotsConfig = serde_json::from_value(json!({
        "type": "Enabled",
        "config": { "repository_key": KEY },
    }))
    .unwrap();
    let encoded = serde_json::to_value(config).unwrap();
    assert_eq!(encoded["type"], "Enabled");
    assert!(matches!(
        serde_json::from_value::<FilesystemSnapshotsConfig>(encoded).unwrap(),
        FilesystemSnapshotsConfig::Enabled(_),
    ));
}

#[test]
fn filesystem_kinds_serialize_as_the_type_tags_of_their_owners() {
    let root: Box<Path> = Path::new("agents").into();
    [
        FilesystemStorageMode::Temporary,
        FilesystemStorageMode::Directory { root: root.clone() },
        FilesystemStorageMode::ManagedXfs {
            root: root.clone(),
            xfs_quotad_socket: root.clone(),
        },
        FilesystemStorageMode::ReflinkXfs { root: root.clone() },
        FilesystemStorageMode::Apfs { root },
    ]
    .into_iter()
    .for_each(|mode| {
        assert_eq!(
            serde_json::to_value(&mode).unwrap()["type"],
            serde_json::to_value(mode.kind()).unwrap(),
            "{mode:?}"
        );
    });
    [
        FilesystemSnapshotsConfig::default(),
        FilesystemSnapshotsConfig::Enabled(Box::new(
            enabled(json!({ "repository_key": KEY })).unwrap(),
        )),
    ]
    .into_iter()
    .for_each(|config| {
        assert_eq!(
            serde_json::to_value(&config).unwrap()["type"],
            serde_json::to_value(config.kind()).unwrap(),
            "{:?}",
            config.kind()
        );
    });
}

#[test]
fn filesystem_snapshots_enabled_config_reads_the_key_the_deadline_and_the_thread_counts() {
    let store = enabled(json!({
        "repository_key": KEY,
        "storage_call_deadline": "45s",
        "restore_reader_threads": 7,
        "save_threads": 3,
    }))
    .unwrap();

    assert_eq!(
        (
            store.repository_key().bytes().to_vec(),
            store.storage_call_deadline(),
            store.restore_reader_threads().get(),
            store.save_threads().get(),
        ),
        ((0..64).collect::<Vec<u8>>(), Duration::from_secs(45), 7, 3)
    );
}

#[test]
fn filesystem_snapshots_enabled_config_gives_the_defaults_of_the_fields_it_does_not_set() {
    let store = enabled(json!({ "repository_key": KEY })).unwrap();

    assert_eq!(
        (
            store.storage_call_deadline(),
            store.restore_reader_threads().get(),
            store.save_threads().get(),
        ),
        (Duration::from_secs(60), 6, 2)
    );
}

#[test]
fn filesystem_snapshots_enabled_config_reads_the_upload_settings() {
    let store = enabled(json!({
        "repository_key": KEY,
        "max_concurrent_uploads": 3,
        "max_concurrent_restores": 5,
        "confirmation_wait": "10s",
        "store_check_limit": "3s",
        "capture_wait": "2s",
        "retained_periodic_snapshots": 4,
        "retained_update_snapshots": 6,
        "max_pending_deletes_per_agent": 9,
        "storage_retry": {
            "max_attempts": 7,
            "min_delay": "1s",
            "max_delay": "30s",
            "multiplier": 2.0,
        },
    }))
    .unwrap();
    let uploads = store.uploads();

    assert_eq!(
        (
            uploads.max_concurrent_uploads().get(),
            uploads.max_concurrent_restores().get(),
            uploads.confirmation_wait(),
            uploads.store_check_limit(),
            uploads.capture_wait(),
            uploads.retained_periodic_snapshots().get(),
            uploads.retained_update_snapshots().get(),
            uploads.max_pending_deletes_per_agent().get(),
            store.storage_retry().clone(),
        ),
        (
            3,
            5,
            Duration::from_secs(10),
            Duration::from_secs(3),
            Duration::from_secs(2),
            4,
            6,
            9,
            RetryConfig {
                max_attempts: 7,
                min_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(30),
                multiplier: 2.0,
                max_jitter_factor: None,
            },
        )
    );
}

#[test]
fn filesystem_snapshots_enabled_config_gives_the_upload_defaults() {
    let store = enabled(json!({ "repository_key": KEY })).unwrap();
    let uploads = store.uploads();

    assert_eq!(
        (
            uploads.max_concurrent_uploads().get(),
            uploads.max_concurrent_restores().get(),
            uploads.confirmation_wait(),
            uploads.store_check_limit(),
            uploads.capture_wait(),
            uploads.retained_periodic_snapshots().get(),
            uploads.retained_update_snapshots().get(),
            uploads.max_pending_deletes_per_agent().get(),
            store.storage_retry().clone(),
        ),
        (
            4,
            8,
            Duration::from_secs(60),
            Duration::from_secs(5),
            Duration::from_secs(5),
            128,
            2,
            1024,
            RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(120),
                multiplier: 4.0,
                max_jitter_factor: None,
            },
        )
    );
}

#[test]
fn filesystem_snapshots_store_settings_accept_a_retry_at_the_edges_and_refuse_one_past_them() {
    let with_retry = |min_delay: u64, max_delay: u64, multiplier: f64| {
        FilesystemSnapshotStoreConfig::new(KEY, Duration::from_secs(60), 1, 1)
            .and_then(|config| {
                config.with_storage_retry(RetryConfig {
                    min_delay: Duration::from_secs(min_delay),
                    max_delay: Duration::from_secs(max_delay),
                    multiplier,
                    ..default_filesystem_snapshot_storage_retry()
                })
            })
            .err()
    };

    assert_eq!(
        [
            with_retry(2, 2, 4.0),
            with_retry(3, 2, 4.0),
            with_retry(2, 120, 1.0),
            with_retry(2, 120, 0.999),
        ],
        [
            None,
            Some("storage_retry.min_delay must not be greater than max_delay".to_string()),
            None,
            Some("storage_retry.multiplier must be at least 1".to_string()),
        ]
    );
}

#[test]
fn filesystem_snapshots_store_settings_refuse_a_jitter_factor_that_is_not_a_number() {
    let with_jitter = |factor: f64| {
        FilesystemSnapshotStoreConfig::new(KEY, Duration::from_secs(60), 1, 1)
            .and_then(|config| {
                config.with_storage_retry(RetryConfig {
                    max_jitter_factor: Some(factor),
                    ..default_filesystem_snapshot_storage_retry()
                })
            })
            .err()
    };
    let refused = "storage_retry.max_jitter_factor must be between 0 and 1".to_string();

    assert_eq!(
        [
            with_jitter(f64::NAN),
            with_jitter(f64::INFINITY),
            with_jitter(0.0),
            with_jitter(1.0),
        ],
        [Some(refused.clone()), Some(refused), None, None]
    );
}

#[test]
fn filesystem_snapshots_enabled_config_refuses_zero_counts_zero_waits_and_a_bad_retry() {
    let retry = |max_attempts: u32, min_delay: &str, max_delay: &str, multiplier: f64| {
        json!({
            "repository_key": KEY,
            "storage_retry": {
                "max_attempts": max_attempts,
                "min_delay": min_delay,
                "max_delay": max_delay,
                "multiplier": multiplier,
            },
        })
    };
    let jitter = |factor: f64| {
        json!({
            "repository_key": KEY,
            "storage_retry": {
                "max_attempts": 3,
                "min_delay": "1s",
                "max_delay": "2s",
                "multiplier": 2.0,
                "max_jitter_factor": factor,
            },
        })
    };
    let cases = [
        (
            json!({ "repository_key": KEY, "max_concurrent_uploads": 0 }),
            "max_concurrent_uploads must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "max_concurrent_restores": 0 }),
            "max_concurrent_restores must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "confirmation_wait": "0s" }),
            "confirmation_wait must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "store_check_limit": "0s" }),
            "store_check_limit must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "capture_wait": "0s" }),
            "capture_wait must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "retained_periodic_snapshots": 0 }),
            "retained_periodic_snapshots must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "retained_update_snapshots": 0 }),
            "retained_update_snapshots must be greater than zero",
        ),
        (
            json!({ "repository_key": KEY, "max_pending_deletes_per_agent": 0 }),
            "max_pending_deletes_per_agent must be greater than zero",
        ),
        (
            retry(0, "1s", "2s", 2.0),
            "storage_retry.max_attempts must be greater than zero",
        ),
        (
            retry(3, "3s", "2s", 2.0),
            "storage_retry.min_delay must not be greater than max_delay",
        ),
        (
            retry(3, "1s", "2s", 0.5),
            "storage_retry.multiplier must be at least 1",
        ),
        (
            jitter(-0.5),
            "storage_retry.max_jitter_factor must be between 0 and 1",
        ),
        (
            jitter(1e20),
            "storage_retry.max_jitter_factor must be between 0 and 1",
        ),
    ];

    let unexpected = cases
        .into_iter()
        .map(|(config, reason)| (refusal(config), reason))
        .filter(|(error, reason)| !error.contains(reason))
        .collect::<Vec<_>>();

    assert_eq!(unexpected, Vec::<(String, &str)>::new());
}

#[test]
fn filesystem_snapshots_enabled_config_accepts_a_deadline_from_one_second_to_an_eighth_of_the_grace()
 {
    let deadline = |text: &str| {
        enabled(json!({ "repository_key": KEY, "storage_call_deadline": text }))
            .map(|store| store.storage_call_deadline())
            .map_err(|error| error.contains("storage_call_deadline must be from 1s to 112.5s"))
    };

    assert_eq!(
        [
            deadline("0s"),
            deadline("999ms"),
            deadline("1s"),
            deadline("60s"),
            deadline("112500ms"),
            deadline("112501ms"),
        ],
        [
            Err(true),
            Err(true),
            Ok(Duration::from_secs(1)),
            Ok(Duration::from_secs(60)),
            Ok(Duration::from_millis(112_500)),
            Err(true),
        ]
    );
}

#[test]
fn filesystem_snapshots_enabled_config_refuses_zero_restore_reader_threads() {
    assert!(
        refusal(json!({ "repository_key": KEY, "restore_reader_threads": 0 }))
            .contains("restore_reader_threads must be greater than zero")
    );
}

#[test]
fn filesystem_snapshots_enabled_config_refuses_zero_save_threads() {
    assert!(
        refusal(json!({ "repository_key": KEY, "save_threads": 0 }))
            .contains("save_threads must be greater than zero")
    );
}

#[test]
fn filesystem_snapshots_enabled_config_refuses_an_empty_short_or_non_hex_key() {
    let non_hex = format!("{}g", &KEY[..127]);
    let cases = [
        (json!({ "repository_key": "" }), "must not be empty"),
        (
            json!({ "repository_key": &KEY[..126] }),
            "must be 128 hex characters",
        ),
        (
            json!({ "repository_key": format!("{KEY}00") }),
            "must be 128 hex characters",
        ),
        (
            json!({ "repository_key": non_hex }),
            "must be 128 hex characters",
        ),
        (json!({}), "missing field `repository_key`"),
    ];

    let unexpected = cases
        .into_iter()
        .map(|(config, reason)| (refusal(config), reason))
        .filter(|(error, reason)| !error.contains(reason))
        .collect::<Vec<_>>();

    assert_eq!(unexpected, Vec::<(String, &str)>::new());
}

#[test]
fn filesystem_snapshots_enabled_config_hides_the_key() {
    let store = enabled(json!({ "repository_key": KEY })).unwrap();
    let shown = FilesystemSnapshotsConfig::Enabled(Box::new(store.clone())).to_safe_string();
    let debugged = format!("{store:?}");

    assert_eq!(
        (
            shown.contains("0001020304"),
            debugged.contains("0001020304"),
            shown.contains("repository key: ****"),
        ),
        (false, false, true)
    );
}
