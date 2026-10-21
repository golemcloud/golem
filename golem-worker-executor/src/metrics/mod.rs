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

// Module holding all the metrics used by the server.
// Collecting them in one place makes it easier to look them up and to share
// common metrics between different layers of the application.

pub(crate) mod resource_release;

use golem_common::golem_version;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref VERSION_INFO: IntCounterVec = register_int_counter_vec!(
        "executor_version_info",
        "Version info of the server",
        &["version", "wasmtime"]
    )
    .unwrap();
}

pub fn register_all() -> Registry {
    VERSION_INFO
        .with_label_values(&[golem_version(), wasmtime::VERSION])
        .inc();

    default_registry().clone()
}

const FUEL_BUCKETS: &[f64; 11] = &[
    1000.0, 10000.0, 25000.0, 50000.0, 100000.0, 250000.0, 500000.0, 1000000.0, 2500000.0,
    5000000.0, 10000000.0,
];

/// Byte-size buckets for scheduled-action payloads and promise completion data:
/// powers of 2 from 1 KB to 64 MB.
const BLOB_SIZE_BUCKETS: &[f64; 17] = &[
    1_024.0,
    2_048.0,
    4_096.0,
    8_192.0,
    16_384.0,
    32_768.0,
    65_536.0,
    131_072.0,
    262_144.0,
    524_288.0,
    1_048_576.0,
    2_097_152.0,
    4_194_304.0,
    8_388_608.0,
    16_777_216.0,
    33_554_432.0,
    67_108_864.0,
];

/// Lag buckets for the scheduler. Values are dense at low latencies so Grafana
/// quantiles distinguish normal scheduling delay from missed ticks.
const SCHEDULER_LAG_BUCKETS: &[f64; 22] = &[
    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
    10.0, 30.0, 60.0, 300.0, 600.0,
];

/// Admission-wait buckets. A worker normally clears admission in milliseconds, but
/// under memory pressure a single phase can block for minutes, so the tail reaches
/// well past the scheduler buckets.
const ADMISSION_WAIT_BUCKETS: &[f64; 18] = &[
    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    1800.0, 3600.0,
];

/// Tick-duration buckets for the scheduler. Values are dense at low latencies
/// because slow ticks directly add scheduling delay.
const SCHEDULER_TICK_DURATION_BUCKETS: &[f64; 24] = &[
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0,
    2.5, 3.0, 5.0, 10.0, 15.0, 20.0, 30.0, 60.0,
];

/// Tick-duration buckets for the oplog sweep. They reach past the shared set's one minute, so
/// ticks against a slow store stay measurable.
const OPLOG_SWEEP_TICK_BUCKETS: &[f64; 9] =
    &[0.01, 0.1, 1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0];

/// Buckets for the deletes and the prunes of filesystem snapshots. A prune that repacks a large
/// repository takes minutes, past the shared set's one minute.
const SNAPSHOT_STORE_BUCKETS: &[f64; 14] = &[
    0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
];

/// Buckets for the age of a restored filesystem snapshot: from one minute to 30 days.
const SNAPSHOT_AGE_BUCKETS: &[f64; 13] = &[
    60.0, 300.0, 900.0, 1800.0, 3600.0, 7200.0, 21600.0, 43200.0, 86400.0, 259200.0, 604800.0,
    1209600.0, 2592000.0,
];

/// Buckets for the size of a single `memory.grow` allocation. Deliberately
/// fine-grained in the 1-32 MiB band where typical guest grows cluster, so
/// that p90/p99 quantiles are not pinned to a coarse 4-16 MiB bucket edge.
const MEMORY_SIZE_BUCKETS: &[f64; 16] = &[
    65536.0,      // 64 KiB
    262144.0,     // 256 KiB
    1048576.0,    // 1 MiB
    2097152.0,    // 2 MiB
    4194304.0,    // 4 MiB
    6291456.0,    // 6 MiB
    8388608.0,    // 8 MiB
    12582912.0,   // 12 MiB
    16777216.0,   // 16 MiB
    25165824.0,   // 24 MiB
    33554432.0,   // 32 MiB
    67108864.0,   // 64 MiB
    134217728.0,  // 128 MiB
    268435456.0,  // 256 MiB
    536870912.0,  // 512 MiB
    1073741824.0, // 1 GiB
];

/// Declares the enum of the values of a metric label, with the value of each variant and the list
/// of all variants.
macro_rules! label_values {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $label:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            const ALL: &'static [Self] = &[$(Self::$variant,)+];

            fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label,)+
                }
            }
        }
    };
}

pub mod component;

pub mod runtime;

pub mod mimalloc;

pub mod events;

pub mod workers;

pub mod promises;

pub mod scheduler;

pub mod sharding;

pub mod wasm;

pub mod oplog;

pub mod resources;

pub mod ephemeral;

pub mod durable_stream;

pub mod storage;

pub mod agent_filesystem;

pub mod filesystem_snapshots;
