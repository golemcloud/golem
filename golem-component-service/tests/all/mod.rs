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

use crate::Tracing;
use golem_service_base::clients::limit::LimitService;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential_suite};

pub mod repo;
pub mod service;
pub mod stubs;

inherit_test_dep!(Tracing);
inherit_test_dep!(Arc<dyn LimitService>);

sequential_suite!(repo);
sequential_suite!(service);
