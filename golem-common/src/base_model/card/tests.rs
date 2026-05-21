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
use chrono::Utc;
use test_r::test;
use uuid::Uuid;

fn fs(owner: &str, recipient: &str, resource: GlobResourcePattern) -> PatternGrant {
    PatternGrant::filesystem_read_pattern(
        owner,
        RecipientPathPattern::parse(recipient).unwrap(),
        resource,
    )
}

fn card(lower_positive: Vec<PatternGrant>, upper_positive: Vec<PatternGrant>) -> Card {
    Card {
        card_id: Uuid::new_v4(),
        parent_ids: Vec::new(),
        lower_positive,
        lower_negative: Vec::new(),
        upper_positive,
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        polymorphic: false,
    }
}

#[test]
fn recipient_depths_are_validated() {
    let valid = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();

    assert!(RecipientPathPattern::parse("acme/shop/prod").is_ok());
    assert!(RecipientPathPattern::parse("acme/*/*").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop/*").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop").is_err());
    assert!(RecipientPathPattern::parse("*/shop/prod").is_err());
    assert!(RecipientPathPattern::parse("*/shop/prod/cart/agent").is_err());
    assert!(RecipientPathPattern::parse("acme/*/prod").is_err());
    assert!(RecipientPathPattern::parse("acme/shop/*/cart/agent").is_err());
    assert!(RecipientPathPattern::parse("acme/*/*/*/*").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop/*/*/*").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop/prod/*/*").is_ok());
    assert!(RecipientPathPattern::parse("acme/shop/prod/cart/*").is_ok());
    assert!(RecipientPathPattern::parse("agent(*)").is_ok());
    assert!(
        RecipientPathPattern::parse("acme/shop/prod/cart/agent")
            .unwrap()
            .subsumes(&valid)
    );
}

#[test]
fn effective_surface_requires_lower_and_all_upper_bounds() {
    let holder = RecipientPathPattern::parse("acme/shop/prod/cart/agent").unwrap();
    let read_all = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::glob("/data/**"),
    );
    let read_secret = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::exact("/data/secret.txt"),
    );
    let read_public = fs(
        "acme/shop/prod/cart/agent",
        "acme/shop/prod/cart/agent",
        GlobResourcePattern::exact("/data/public.txt"),
    );

    let lower = card(vec![read_all], Vec::new());
    let ceiling = card(Vec::new(), vec![read_public.clone()]);
    let surface = EffectiveSurface::from_cards(&[lower, ceiling], &holder).unwrap();

    assert!(surface.authorize(&read_public).unwrap());
    assert!(!surface.authorize(&read_secret).unwrap());
}
