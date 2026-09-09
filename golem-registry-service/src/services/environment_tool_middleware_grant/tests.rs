// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use super::*;
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::card::{EffectiveSurface, GrantSurface};
use golem_common::model::environment::{EnvironmentName, EnvironmentRevision};
use test_r::test;

fn test_environment() -> Environment {
    Environment {
        id: EnvironmentId::new(),
        revision: EnvironmentRevision::INITIAL,
        application_id: ApplicationId::new(),
        application_name: ApplicationName::try_from("app").unwrap(),
        name: EnvironmentName::try_from("dev").unwrap(),
        diff_model_version: 0,
        compatibility_check: false,
        version_check: false,
        security_overrides: false,
        owner_account_id: AccountId::new(),
        owner_account_email: golem_common::model::account::AccountEmail::new("owner@example.com"),
        current_deployment: None,
    }
}

fn view_permission(environment: &Environment, name: ToolMiddlewareName) -> PermissionTarget {
    PermissionTarget::EnvironmentToolMiddlewareGrant(ClassPermissionTarget {
        verb: Some(EnvironmentToolMiddlewareGrantVerb::View),
        owner: environment_owner(environment),
        resource: EnvironmentToolMiddlewareGrantResourcePattern::Name(name),
    })
}

#[test]
fn name_scoped_view_permission_authorizes_only_that_granted_tool_middleware() {
    let environment = test_environment();
    let permitted = ToolMiddlewareName::try_from("search").unwrap();
    let denied = ToolMiddlewareName::try_from("payments").unwrap();
    let auth = AuthCtx::agent_with_effective_surface(
        environment.owner_account_id,
        environment.owner_account_email.clone(),
        EffectiveSurface {
            source_card_ids: Vec::new(),
            lower: vec![GrantSurface {
                positive: vec![view_permission(&environment, permitted.clone())],
                negative: Vec::new(),
            }],
            upper: Vec::new(),
        },
    );

    assert!(
        authorize_environment_tool_middleware_grant_permission(
            &auth,
            &environment,
            EnvironmentToolMiddlewareGrantVerb::View,
            permitted
        )
        .is_ok()
    );
    assert!(
        authorize_environment_tool_middleware_grant_permission(
            &auth,
            &environment,
            EnvironmentToolMiddlewareGrantVerb::View,
            denied
        )
        .is_err()
    );
}
