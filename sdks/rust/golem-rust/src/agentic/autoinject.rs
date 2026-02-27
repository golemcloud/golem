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

use super::{Config, ConfigEntry, ConfigSchema};

#[diagnostic::on_unimplemented(
    message = "Only autoinjectable types are allowed to be annotated with the autoinject attribute. Supported\n\
                      types are `golem_rust::agentic::Config`"
)]
pub trait AutoInjectable: Sized {
    fn config_entries() -> Vec<ConfigEntry>;

    // TODO: add more ambient values values available during agent initialization like principal here.
    fn autoinject() -> Result<Self, String>;
}

impl<T: ConfigSchema> AutoInjectable for Config<T> {
    fn config_entries() -> Vec<ConfigEntry> {
        T::describe_config()
    }

    fn autoinject() -> Result<Self, String> {
        T::load(&[]).map(Config)
    }
}
