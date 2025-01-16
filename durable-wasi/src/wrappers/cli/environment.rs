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

use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::cli::environment::{get_arguments, get_environment, initial_cwd};
use crate::durability::Durability;
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::cli::environment::Guest for crate::Component {
    fn get_environment() -> Vec<(String, String)> {
        let durability = Durability::<Vec<(String, String)>, SerializableError>::new(
            "golem_environment",
            "get_environment",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = get_environment();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }

    fn get_arguments() -> Vec<String> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            "golem_environment",
            "get_arguments",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = get_arguments();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }

    fn initial_cwd() -> Option<String> {
        let durability = Durability::<Option<String>, SerializableError>::new(
            "golem_environment",
            "get_arguments", // TODO: fix in 2.0 - for backward compatibility with Golem 1.0
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = initial_cwd();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
