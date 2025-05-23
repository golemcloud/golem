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

pub mod api_tags;
pub mod auth;
pub mod config;
pub mod db;
pub mod headers;
pub mod metrics;
pub mod migration;
pub mod model;
pub mod observability;
pub mod poem;
pub mod replayable_stream;
pub mod repo;
pub mod service;
pub mod storage;
pub mod stream;

#[cfg(test)]
test_r::enable!();
