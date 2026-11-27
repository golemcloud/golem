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
use golem_common::metrics::catalog;
use prometheus::{IntGaugeVec, Registry, default_registry};
use std::sync::LazyLock;

static BUILD_INFO: LazyLock<IntGaugeVec> =
    LazyLock::new(|| catalog::EXECUTOR_BUILD_INFO.register());

pub fn register_all() -> Registry {
    BUILD_INFO
        .with_label_values(&[golem_version(), wasmtime::VERSION])
        .set(1);

    default_registry().clone()
}

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
