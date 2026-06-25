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

pub mod sdk {
    pub const RUST: &str = "2.1.0";
    pub const TS: &str = "1.1.1";
    pub const SCALA: &str = "1.5.1";
    pub const MOONBIT: &str = "0.5.2";
    pub const MANIFEST: &str = "1.6.0";

    #[macro_export]
    macro_rules! manifest_schema_version {
        () => {
            "1.6.0-dev.3"
        };
    }
}

pub mod build_tool {
    pub const RUSTUP_MIN: &str = "1.27.1";
    pub const RUSTC_MIN: &str = "1.94.0";
    pub const CARGO_MIN: &str = "1.94.0";
    pub const NODE_MIN: &str = "24.11.0";
    pub const NPM_MIN: &str = "11.6.2";
    pub const MOON_MIN: &str = "0.1.20260409";
    pub const JAVA_MIN: &str = "17.0.0";
    pub const WASM_TOOLS_MIN: &str = "1.227.1";
}

// Keep this aligned with the TS component template
pub mod ts_dep {
    pub const ROLLUP_PLUGIN_ALIAS: &str = "^5.1.1";
    pub const ROLLUP_PLUGIN_NODE_RESOLVE: &str = "^16.0.1";
    pub const ROLLUP_PLUGIN_TYPESCRIPT: &str = "^12.1.4";
    pub const ROLLUP_PLUGIN_COMMONJS: &str = "^28.0.6";
    pub const ROLLUP_PLUGIN_JSON: &str = "^6.1.0";
    pub const TYPES_NODE: &str = "^24.3.1";
    pub const ROLLUP: &str = "^4.50.1";
    pub const TSLIB: &str = "^2.8.1";
    pub const TYPESCRIPT: &str = "^5.9.2";
}

// Keep this aligned with the Rust component template
pub mod rust_dep {
    pub const LOG: &str = "0.4.29";
    pub const SERDE: &str = "1";
    pub const SERDE_JSON: &str = "1";
    pub const WSTD: &str = "=0.6.5";
}

pub mod scala_dep {
    pub const SCALA_VERSION: &str = "3.8.2";
    pub const SCALA_2_VERSION: &str = "2.13.18";
    pub const SCALAJS_PLUGIN_VERSION: &str = "1.20.2";
    pub const SBT_VERSION: &str = "1.12.0";
    /// zio-blocks (schema/JSON) library used by the generated Scala bridge
    /// client runtime for JSON parsing and rendering.
    pub const ZIO_BLOCKS_VERSION: &str = "0.0.43";
}

pub mod moonbit_dep {
    pub const ASYNC_VERSION: &str = "0.18.1";
}

pub mod ai {
    pub const VERSION: &str = "v0.5.2";
    pub const SUFFIX: &str = ".wasm";
}
