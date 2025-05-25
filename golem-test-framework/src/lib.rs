// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

/// Individual components of an integration test such as services, databases etc.
pub mod components;

/// Different ways to configure the launch of test components
pub mod config;

/// Higher level DSL for expressing common steps of tests and benchmarks
pub mod dsl;

pub mod model;

pub type Result<T> = anyhow::Result<T>;

#[cfg(test)]
test_r::enable!();
