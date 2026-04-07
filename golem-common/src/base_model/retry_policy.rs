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

use serde::{Deserialize, Serialize};

// --- ApiPredicateValue ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiTextValue {
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiIntegerValue {
    pub value: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiBooleanValue {
    pub value: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ApiPredicateValue {
    Text(ApiTextValue),
    Integer(ApiIntegerValue),
    Boolean(ApiBooleanValue),
}

// --- ApiPredicate ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyComparison {
    pub property: String,
    pub value: ApiPredicateValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyExistence {
    pub property: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertySetCheck {
    pub property: String,
    pub values: Vec<ApiPredicateValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyPattern {
    pub property: String,
    pub pattern: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyPrefix {
    pub property: String,
    pub prefix: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertySubstring {
    pub property: String,
    pub substring: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicatePair {
    pub left: Box<ApiPredicate>,
    pub right: Box<ApiPredicate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateNot {
    pub predicate: Box<ApiPredicate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateTrue {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateFalse {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ApiPredicate {
    PropEq(ApiPropertyComparison),
    PropNeq(ApiPropertyComparison),
    PropGt(ApiPropertyComparison),
    PropGte(ApiPropertyComparison),
    PropLt(ApiPropertyComparison),
    PropLte(ApiPropertyComparison),
    PropExists(ApiPropertyExistence),
    PropIn(ApiPropertySetCheck),
    PropMatches(ApiPropertyPattern),
    PropStartsWith(ApiPropertyPrefix),
    PropContains(ApiPropertySubstring),
    And(ApiPredicatePair),
    Or(ApiPredicatePair),
    Not(ApiPredicateNot),
    True(ApiPredicateTrue),
    False(ApiPredicateFalse),
}

// --- ApiRetryPolicy ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPeriodicPolicy {
    pub delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiExponentialPolicy {
    pub base_delay_ms: u64,
    pub factor: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiFibonacciPolicy {
    pub first_ms: u64,
    pub second_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiImmediatePolicy {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiNeverPolicy {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiCountBoxPolicy {
    pub max_retries: u32,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiTimeBoxPolicy {
    pub limit_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiClampPolicy {
    pub min_delay_ms: u64,
    pub max_delay_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiAddDelayPolicy {
    pub delay_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiJitterPolicy {
    pub factor: f64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiFilteredOnPolicy {
    pub predicate: ApiPredicate,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiRetryPolicyPair {
    pub first: Box<ApiRetryPolicy>,
    pub second: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ApiRetryPolicy {
    Periodic(ApiPeriodicPolicy),
    Exponential(ApiExponentialPolicy),
    Fibonacci(ApiFibonacciPolicy),
    Immediate(ApiImmediatePolicy),
    Never(ApiNeverPolicy),
    CountBox(ApiCountBoxPolicy),
    TimeBox(ApiTimeBoxPolicy),
    Clamp(ApiClampPolicy),
    AddDelay(ApiAddDelayPolicy),
    Jitter(ApiJitterPolicy),
    FilteredOn(ApiFilteredOnPolicy),
    AndThen(ApiRetryPolicyPair),
    Union(ApiRetryPolicyPair),
    Intersect(ApiRetryPolicyPair),
}

// ── Registry CRUD DTO types ──────────────────────────────────────────

use crate::{declare_revision, declare_structs, newtype_uuid};

newtype_uuid!(RetryPolicyId);

declare_revision!(RetryPolicyRevision);

declare_structs! {
    pub struct RetryPolicyDto {
        pub id: RetryPolicyId,
        pub environment_id: crate::base_model::environment::EnvironmentId,
        pub name: String,
        pub revision: RetryPolicyRevision,
        pub priority: u32,
        pub predicate_json: String,
        pub policy_json: String,
    }

    pub struct RetryPolicyCreation {
        pub name: String,
        pub priority: u32,
        pub predicate_json: String,
        pub policy_json: String,
    }

    pub struct RetryPolicyUpdate {
        pub current_revision: RetryPolicyRevision,
        pub priority: Option<u32>,
        pub predicate_json: Option<String>,
        pub policy_json: Option<String>,
    }
}


