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

mod client;
mod connect;
mod connect_proxy;
mod error;
mod routing_logic;
mod service;
mod worker_stream;

pub use client::*;
pub use connect::*;
pub use connect_proxy::*;
pub use error::*;
pub use routing_logic::*;
pub use service::*;
pub use worker_stream::*;

pub type WorkerResult<T> = Result<T, WorkerServiceError>;
