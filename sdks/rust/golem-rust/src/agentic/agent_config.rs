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

use super::schema::Schema;
use crate::golem_agentic::golem::agent::common::{ConfigKeyValueType, ConfigValueType};
use crate::golem_agentic::golem::agent::host::get_config_value;
use golem_wasm::WitType;
use std::marker::PhantomData;

pub trait ConfigSchema: Sized {
    fn describe_config() -> Vec<ConfigEntry>;
    fn load() -> Result<Self, String>;
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
        T: Schema,
    {
        let value = get_config_value(&self.path);
        T::from_wit_value(value, T::get_type())
    }
}

#[derive(Clone)]
pub struct ConfigEntry {
    pub key: String,
    pub shared: bool,
    pub schema: WitType,
}

impl From<ConfigEntry> for ConfigKeyValueType {
    fn from(value: ConfigEntry) -> Self {
        if value.shared {
            ConfigKeyValueType {
                key: vec![value.key],
                value: ConfigValueType::Shared(value.schema),
            }
        } else {
            ConfigKeyValueType {
                key: vec![value.key],
                value: ConfigValueType::Local(value.schema),
            }
        }
    }
}

pub trait ConfigField: Sized {
    type Inner: Schema;

    const IS_SHARED: bool;

    fn load_from_path(path: &[String]) -> Result<Self, String>;
}

impl<T: Schema> ConfigField for T {
    type Inner = T;

    const IS_SHARED: bool = false;

    fn load_from_path(path: &[String]) -> Result<Self, String> {
        let value = get_config_value(path);
        T::from_wit_value(value, T::get_type())
    }
}

impl<T: Schema> ConfigField for Secret<T> {
    type Inner = T;

    const IS_SHARED: bool = true;

    fn load_from_path(path: &[String]) -> Result<Self, String> {
        Ok(Secret::new(path.to_vec()))
    }
}
