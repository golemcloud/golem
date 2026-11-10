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

//! The caller model: who started an invocation chain, and who made this call.
//!
//! Every identity is an `(issuer, subject)` [`Pair`]. A [`Principal`] is one identity with the
//! details that Golem verified for it, a [`PrincipalRef`] is a stored pair that grants nothing,
//! and a [`Caller`] holds the original and the immediate principal of one invocation, with the
//! makers of the tokens that the request presented.
//!
//! The golden vectors in `test-data/caller-identity/` fix the text form of a pair, the Golem
//! subjects and the identity-proof format, for every implementation to test against.
//!
//! Invocations still carry the principal of `base_model::agent`; this model replaces it.

mod identity_proof;
mod pair;
mod principal;
mod subject;

pub use identity_proof::*;
pub use pair::*;
pub use principal::*;
pub use subject::*;

#[cfg(test)]
mod conversion_from_1_6_tests;

#[cfg(test)]
mod identity_proof_tests;

#[cfg(test)]
mod principal_tests;

#[cfg(test)]
mod text_form_tests;
