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

/// Top level commands for generating stub WASM components
pub mod generate;

/// Top level commands for managing dependencies on stubs
pub mod dependencies;

/// Top level commands for composing stubs with user components
pub mod composition;

/// Top level commands for adding metadata to WASM components
pub mod metadata;
