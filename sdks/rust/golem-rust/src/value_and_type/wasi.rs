// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::type_builder::TypeNodeBuilder;
use crate::bindings::wasi::logging::logging::Level;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::{NodeBuilder, WitValueExtractor};

impl IntoValue for Level {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            Level::Trace => 0,
            Level::Debug => 1,
            Level::Info => 2,
            Level::Warn => 3,
            Level::Error => 4,
            Level::Critical => 5,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.variant(Some("Level".to_string()), Some("wasi".to_string()));
        builder = builder.unit_case("trace");
        builder = builder.unit_case("debug");
        builder = builder.unit_case("info");
        builder = builder.unit_case("warn");
        builder = builder.unit_case("error");
        builder = builder.unit_case("critical");
        builder.finish()
    }
}

impl FromValueAndType for Level {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected Level to be a variant".to_string())?;
        if inner.is_some() {
            return Err("Level variants should not have values".to_string());
        }
        match idx {
            0 => Ok(Level::Trace),
            1 => Ok(Level::Debug),
            2 => Ok(Level::Info),
            3 => Ok(Level::Warn),
            4 => Ok(Level::Error),
            5 => Ok(Level::Critical),
            _ => Err(format!("Invalid Level variant index: {}", idx)),
        }
    }
}

impl IntoValue for wasi::io::error::Error {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.string(&self.to_debug_string())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.string()
    }
}

impl FromValueAndType for wasi::io::error::Error {
    fn from_extractor<'a, 'b>(
        _extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Err("Not supported".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roundtrip_test;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_level,
        Level,
        (0u32..=5).prop_map(|i| {
            match i {
                0 => Level::Trace,
                1 => Level::Debug,
                2 => Level::Info,
                3 => Level::Warn,
                4 => Level::Error,
                _ => Level::Critical,
            }
        })
    );
}
