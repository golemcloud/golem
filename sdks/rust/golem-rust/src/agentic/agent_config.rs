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

use crate::golem_agentic::golem::agent::common::{ConfigKeyValueType, ConfigValueType};
use crate::golem_agentic::golem::agent::host::get_config_value;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm::WitType;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::num::{
    NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8,
};
use std::ops::{Bound, Range};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub struct Config<T>(pub T);

pub trait ConfigSchema: Sized {
    fn describe_config() -> Vec<ConfigEntry>;
    fn load(path: &[String]) -> Result<Self, String>;
}

pub struct Secret<T> {
    path: Vec<String>,
    config_type: PhantomData<T>,
}

impl<T> Secret<T> {
    pub fn new(path: Vec<String>) -> Self {
        Self {
            path,
            config_type: PhantomData::<T>,
        }
    }

    pub fn get(&self) -> Result<T, String>
    where
        T: FromValueAndType + IntoValue,
    {
        let value = get_config_value(&self.path);
        T::from_value_and_type(ValueAndType {
            value,
            typ: T::get_type(),
        })
    }
}

#[derive(Clone)]
pub struct ConfigEntry {
    pub key: Vec<String>,
    pub shared: bool,
    pub schema: WitType,
}

impl From<ConfigEntry> for ConfigKeyValueType {
    fn from(value: ConfigEntry) -> Self {
        if value.shared {
            ConfigKeyValueType {
                key: value.key,
                value: ConfigValueType::Shared(value.schema),
            }
        } else {
            ConfigKeyValueType {
                key: value.key,
                value: ConfigValueType::Local(value.schema),
            }
        }
    }
}

#[diagnostic::on_unimplemented(message = "\
        `ConfigField` is not implemented for `{Self}`. Only types that implement `ConfigField` can be\n\
        used as part of an agent's config. If you tried to use a struct as part of the config, make sure\n\
        it derives ConfigSchema.")]
pub trait ConfigField: Sized {
    const IS_SHARED: bool;

    fn collect_entries(path: &[String]) -> Vec<ConfigEntry>;
    fn load(path: &[String]) -> Result<Self, String>;
}

impl<T: IntoValue> ConfigField for Secret<T> {
    const IS_SHARED: bool = true;

    fn collect_entries(path_prefix: &[String]) -> Vec<ConfigEntry> {
        vec![ConfigEntry {
            key: path_prefix.to_vec(),
            shared: true,
            schema: T::get_type(),
        }]
    }

    fn load(path: &[String]) -> Result<Self, String> {
        Ok(Secret::new(path.to_vec()))
    }
}

// Marker for component model types that can be used as leaf config nodes
pub trait ComponentModelConfigLeaf: IntoValue + FromValueAndType {}

impl<T: ComponentModelConfigLeaf> ConfigField for T {
    const IS_SHARED: bool = false;

    fn collect_entries(path_prefix: &[String]) -> Vec<ConfigEntry> {
        vec![ConfigEntry {
            key: path_prefix.to_vec(),
            shared: false,
            schema: <Self as IntoValue>::get_type(),
        }]
    }

    fn load(path: &[String]) -> Result<Self, String> {
        let value = get_config_value(path);
        <Self as FromValueAndType>::from_value_and_type(ValueAndType {
            value,
            typ: <Self as IntoValue>::get_type(),
        })
    }
}

macro_rules! impl_component_model_config_leaf {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ComponentModelConfigLeaf for $ty {}
        )+
    };
}

impl_component_model_config_leaf![
    bool,
    char,
    f32,
    f64,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
    usize,
    String,
    Duration,
    IpAddr,
    NonZeroI8,
    NonZeroI16,
    NonZeroI32,
    NonZeroI64,
    NonZeroU8,
    NonZeroU16,
    NonZeroU32,
    NonZeroU64,
    uuid::Uuid,
    crate::bindings::golem::api::host::AgentAllFilter,
    crate::bindings::golem::api::host::AgentAnyFilter,
    crate::bindings::golem::api::host::AgentConfigVarsFilter,
    crate::bindings::golem::api::host::AgentCreatedAtFilter,
    crate::bindings::golem::api::host::AgentEnvFilter,
    crate::bindings::golem::api::host::AgentMetadata,
    crate::bindings::golem::api::host::AgentNameFilter,
    crate::bindings::golem::api::host::AgentPropertyFilter,
    crate::bindings::golem::api::host::AgentStatus,
    crate::bindings::golem::api::host::AgentStatusFilter,
    crate::bindings::golem::api::host::AgentVersionFilter,
    crate::bindings::golem::api::host::FilterComparator,
    crate::bindings::golem::api::host::PromiseId,
    crate::bindings::golem::api::host::StringFilterComparator,
    crate::bindings::golem::api::host::UpdateMode,
    crate::bindings::wasi::logging::logging::Level,
];

#[cfg(feature = "bigdecimal")]
impl_component_model_config_leaf![bigdecimal::BigDecimal,];

#[cfg(feature = "bit_vec")]
impl_component_model_config_leaf![bit_vec::BitVec,];

#[cfg(feature = "bytes")]
impl_component_model_config_leaf![bytes::Bytes,];

#[cfg(feature = "chrono")]
impl_component_model_config_leaf![
    chrono::DateTime<chrono::FixedOffset>,
    chrono::DateTime<chrono::Local>,
    chrono::DateTime<chrono::Utc>,
    chrono::FixedOffset,
    chrono::Month,
    chrono::NaiveDate,
    chrono::NaiveDateTime,
    chrono::NaiveTime,
    chrono::Weekday,
];

#[cfg(feature = "mac_address")]
impl_component_model_config_leaf![mac_address::MacAddress,];

#[cfg(feature = "num_bigint")]
impl_component_model_config_leaf![num_bigint::BigInt,];

#[cfg(feature = "rust_decimal")]
impl_component_model_config_leaf![rust_decimal::Decimal,];

#[cfg(feature = "serde_json_types")]
impl_component_model_config_leaf![serde_json::Value,];

#[cfg(feature = "url")]
impl_component_model_config_leaf![url::Url,];

#[cfg(feature = "nonempty_collections")]
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for nonempty_collections::NEVec<T> {}

macro_rules! impl_component_model_config_leaf_for_tuple {
    ($($T:ident),+) => {
        impl<$($T: IntoValue + FromValueAndType),+> ComponentModelConfigLeaf for ($($T),+) {}
    };
}

macro_rules! impl_component_model_config_leaf_for_tuples {
    ($first:ident, $second:ident $(,$rest:ident)*) => {
        impl_component_model_config_leaf_for_tuple!($first, $second $(,$rest)*);
        impl_component_model_config_leaf_for_tuples!($second $(,$rest)*);
    };
    ($single:ident) => {};
    () => {};
}

impl_component_model_config_leaf_for_tuples!(A, B, C, D, E, F, G, H, I, J, K, L);

impl<S: IntoValue + FromValueAndType, E: IntoValue + FromValueAndType> ComponentModelConfigLeaf
    for Result<S, E>
{
}
impl<E: IntoValue + FromValueAndType> ComponentModelConfigLeaf for Result<(), E> {}
impl<S: IntoValue + FromValueAndType> ComponentModelConfigLeaf for Result<S, ()> {}
impl ComponentModelConfigLeaf for Result<(), ()> {}

impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for Option<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for Bound<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for Range<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for Vec<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for VecDeque<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for LinkedList<T> {}
impl<T: FromValueAndType + IntoValue> ComponentModelConfigLeaf for Box<T> {}
impl<T: FromValueAndType + IntoValue + Clone> ComponentModelConfigLeaf for Rc<T> {}
impl<T: FromValueAndType + IntoValue + Clone> ComponentModelConfigLeaf for Arc<T> {}

impl<T: FromValueAndType + IntoValue + Hash + Eq> ComponentModelConfigLeaf for HashSet<T> {}
impl<T: FromValueAndType + IntoValue + Ord> ComponentModelConfigLeaf for BTreeSet<T> {}

impl<K: FromValueAndType + IntoValue + Hash + Eq, V: FromValueAndType + IntoValue>
    ComponentModelConfigLeaf for HashMap<K, V>
{
}
impl<K: FromValueAndType + IntoValue + Ord, V: FromValueAndType + IntoValue>
    ComponentModelConfigLeaf for BTreeMap<K, V>
{
}
