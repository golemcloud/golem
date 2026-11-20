// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! The decision API: the one question that every entry point and host function asks.
//!
//! An [`AuthorizationRequest`] names the principal whose authority decides, the caller, the
//! authority that the request brings, and one [`Operation`] of the catalog. An operation has one
//! or more parts, and each part has a verb, a resource and its own [`Target`]. The answer is a
//! [`Decision`]: allow, with the cards that covered the operation, or deny, in the shape that the
//! response must take.
//!
//! These types hold no evaluation. The `Authorizer` trait of `golem-service-base` decides.

mod class;
mod decision;
mod operation;
mod request;
mod resource;
mod target;

pub use class::*;
pub use decision::*;
pub use operation::*;
pub use request::*;
pub use resource::*;
pub use target::*;

#[cfg(test)]
mod tests;
