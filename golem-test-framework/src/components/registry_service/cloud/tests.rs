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

use super::*;
use test_r::test;

#[test]
fn cloud_has_no_suite_quota_plans() {
    let service = CloudRegistryService::new(
        Url::parse("https://fixture.invalid:18481").unwrap(),
        TokenSecret::new(),
        AccountId(uuid::Uuid::from_u128(101)),
        PlanId(uuid::Uuid::from_u128(211)),
    );
    let registry: &dyn RegistryService = &service;
    assert_eq!(
        [
            registry.low_fuel_plan(),
            registry.low_disk_space_plan(),
            registry.low_http_calls_plan(),
            registry.low_rpc_calls_plan(),
        ],
        [None; 4]
    );
    assert_eq!(registry.default_plan().0, uuid::Uuid::from_u128(211));
}

/// Every generated client has to reach the gateway, not `http://host:port`.
///
/// This is a regression test with a cost attached: `resources_client` was
/// added while only `client()` was overridden here, so it built
/// `http://release.dev-api.golem.cloud:443`, got an empty body back, and
/// failed chaos-prep with a JSON decode error that named neither the URL nor
/// the cause. Overriding the *context* is what makes one fix cover every
/// client, and this asserts the override is still in place.
#[test]
async fn every_client_reaches_the_gateway_url() {
    let api_url = Url::parse("https://release.dev-api.golem.cloud").unwrap();
    let service = CloudRegistryService::new(
        api_url.clone(),
        TokenSecret::new(),
        AccountId(uuid::Uuid::nil()),
        PlanId(uuid::Uuid::nil()),
    );
    let token = TokenSecret::new();

    assert_eq!(service.client_context(&token).await.base_url, api_url);
    assert_eq!(service.client(&token).await.context.base_url, api_url);
    assert_eq!(
        service.resources_client(&token).await.context.base_url,
        api_url,
        "a client that skips the context override silently talks to the wrong host"
    );
}
