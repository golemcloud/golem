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

use super::subsumption_tests as subsumption;
use super::*;
use crate::base_model::authority::{
    self as v2, AgentTarget, CatalogOperation, Decision, Denial, DesignatedKind, GrantorCard,
    Grantors, Operation, Resource, Target, ToolTarget,
};
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, AgentId};
use crate::model::agent::AgentTypeName;
use crate::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use crate::model::card::recipient::RecipientPattern;
use crate::model::environment::EnvironmentId;
use chrono::Utc;
use test_r::core::{DynamicTestRegistration, TestProperties};
use test_r::{add_test, test, test_gen};
use uuid::Uuid;

const OWNER: &str = "acme/shop/prod/cart";

fn agent_id() -> AgentId {
    AgentId::from_parts(7, 1)
}

fn agent_target() -> AgentTarget {
    AgentTarget {
        environment_id: EnvironmentId(Uuid::from_u128(0xe1)),
        agent_type: AgentTypeName("Cart".to_string()),
        agent_id: agent_id(),
        fingerprint: Some(AgentFingerprint(Uuid::from_u128(0xf1))),
    }
}

fn tool_target() -> ToolTarget {
    ToolTarget {
        environment_id: EnvironmentId(Uuid::from_u128(0xe1)),
        tool_name: ToolName::try_from("bash").unwrap(),
    }
}

/// The 1.6 owner of the agent of `agent_target`.
fn agent_owner() -> AgentOwnerPattern {
    AgentOwnerPattern::parse(&format!("{OWNER}/{}", agent_id())).unwrap()
}

fn grant(text: &str) -> PermissionPattern {
    parse_permission(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// A concrete 1.6 check, written as grant text with the agent's own owner.
fn check(class: &str, verb: &str, resource: &str) -> PermissionTarget {
    grant(&format!(
        "{class}({OWNER}/{}) @ acme : {verb} : {resource}",
        agent_id()
    ))
    .to_target()
}

fn holder() -> RecipientPattern {
    RecipientPattern::parse("acme").unwrap()
}

fn surface(lower: &[&str], lower_negative: &[&str], upper: &[&str]) -> EffectiveSurface {
    let parse = |texts: &[&str]| texts.iter().map(|text| grant(text)).collect::<Vec<_>>();
    EffectiveSurface::from_grants(
        &parse(lower),
        &parse(lower_negative),
        &parse(upper),
        &[],
        &holder(),
    )
    .unwrap()
}

fn card_with(lower: &[&str]) -> Card {
    Card {
        card_id: CardId::new(),
        parent_ids: Vec::new(),
        lower_positive: lower.iter().map(|text| grant(text)).collect(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

fn single(name: CatalogOperation, resource: Resource) -> Operation {
    Operation::single(name, resource, Target::Agent(agent_target())).unwrap()
}

fn hosted_exec() -> Operation {
    Operation::hosted_exec(agent_target(), tool_target(), "bash".to_string()).unwrap()
}

fn is_allowed(decision: &Decision) -> bool {
    matches!(decision, Decision::Allow(_))
}

/// One 1.6 case that the subsumption tests do not hold as a table: the grants of a surface, the
/// operation that the check belongs to, and today's 1.6 check. These cases add upper bounds,
/// negative grants and oplog ranges to the shared tables below.
struct Case {
    name: &'static str,
    lower: &'static [&'static str],
    lower_negative: &'static [&'static str],
    upper: &'static [&'static str],
    operation: fn() -> Operation,
    check: fn() -> PermissionTarget,
}

fn invoke_increment() -> Operation {
    single(
        CatalogOperation::InvokeAgent,
        Resource::Method("increment".to_string()),
    )
}

fn read_oplog() -> Operation {
    single(CatalogOperation::ReadAgentOplog, Resource::None)
}

fn read_file() -> Operation {
    single(
        CatalogOperation::ReadFile,
        Resource::Path("/data/a.txt".to_string()),
    )
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "invoke_star_allows_a_method",
            lower: &["agent(acme/shop/prod/cart/*) @ acme : invoke : *"],
            lower_negative: &[],
            upper: &[],
            operation: invoke_increment,
            check: || check("agent", "invoke", "increment"),
        },
        Case {
            name: "invoke_of_another_method_denies",
            lower: &["agent(acme/shop/prod/cart/*) @ acme : invoke : decrement"],
            lower_negative: &[],
            upper: &[],
            operation: invoke_increment,
            check: || check("agent", "invoke", "increment"),
        },
        Case {
            name: "another_component_denies",
            lower: &["agent(acme/shop/prod/billing/*) @ acme : invoke : *"],
            lower_negative: &[],
            upper: &[],
            operation: invoke_increment,
            check: || check("agent", "invoke", "increment"),
        },
        Case {
            name: "a_negative_grant_denies",
            lower: &["agent(acme/shop/prod/cart/*) @ acme : invoke : *"],
            lower_negative: &["agent(acme/shop/prod/cart/*) @ acme : invoke : increment"],
            upper: &[],
            operation: invoke_increment,
            check: || check("agent", "invoke", "increment"),
        },
        Case {
            name: "an_upper_bound_without_the_method_denies",
            lower: &["agent(acme/shop/prod/cart/*) @ acme : invoke : *"],
            lower_negative: &[],
            upper: &["agent(acme/shop/prod/cart/*) @ acme : view : *"],
            operation: invoke_increment,
            check: || check("agent", "invoke", "increment"),
        },
        Case {
            name: "an_oplog_range_inside_the_grant_allows",
            lower: &["oplog(acme/shop/prod/cart/*) @ acme : read : start=0:end=100"],
            lower_negative: &[],
            upper: &[],
            operation: read_oplog,
            check: || check("oplog", "read", "start=10:end=20"),
        },
        Case {
            name: "an_oplog_range_outside_the_grant_denies",
            lower: &["oplog(acme/shop/prod/cart/*) @ acme : read : start=0:end=100"],
            lower_negative: &[],
            upper: &[],
            operation: read_oplog,
            check: || check("oplog", "read", "start=90:end=120"),
        },
        Case {
            name: "a_path_glob_allows_a_file",
            lower: &["filesystem(acme/shop/prod/cart/*) @ acme : read : /data/**"],
            lower_negative: &[],
            upper: &[],
            operation: read_file,
            check: || check("filesystem", "read", "/data/a.txt"),
        },
        Case {
            name: "a_path_glob_elsewhere_denies",
            lower: &["filesystem(acme/shop/prod/cart/*) @ acme : read : /etc/**"],
            lower_negative: &[],
            upper: &[],
            operation: read_file,
            check: || check("filesystem", "read", "/data/a.txt"),
        },
    ]
}

#[test_gen]
fn generate_compatibility_equals_effective_surface_tests(r: &mut DynamicTestRegistration) {
    for case in cases() {
        add_test!(
            r,
            format!("compatibility_equals_effective_surface_{}", case.name),
            TestProperties::unit_test(),
            move || {
                let surface = surface(case.lower, case.lower_negative, case.upper);
                let check = (case.check)();
                let today = surface.authorize(&check).unwrap();
                let decision = decide_with_checks(&surface, &(case.operation)(), &[vec![check]]);
                assert_eq!(is_allowed(&decision), today, "{decision:?}");
            }
        );
    }
}

/// Today's answer and the decision's answer for one check of `operation` on `surface`.
fn answers(
    surface: &EffectiveSurface,
    operation: &Operation,
    check: PermissionTarget,
) -> (bool, bool) {
    let today = surface.authorize(&check).unwrap();
    let decision = decide_with_checks(surface, operation, &[vec![check]]);
    (today, is_allowed(&decision))
}

fn surface_of(grant: PermissionPattern, holder: &RecipientPattern) -> EffectiveSurface {
    EffectiveSurface::from_grants(&[grant], &[], &[], &[], holder).unwrap()
}

/// The owner cases of the subsumption tests, as a `view` grant and a `view` check: today's check
/// allows exactly where the owner of the grant covers the owner of the request, and so does the
/// decision.
#[test_gen]
fn generate_decision_over_owner_subsumption_tests(r: &mut DynamicTestRegistration) {
    for (granted, requested, covers) in subsumption::owner_subsumption_cases() {
        add_test!(
            r,
            format!(
                "decision_over_owner_subsumption_{}_{}",
                subsumption::test_name(granted),
                subsumption::test_name(requested)
            ),
            TestProperties::unit_test(),
            move || {
                let grant = PermissionPattern::Agent(ClassPermissionPattern {
                    verb: Some(AgentVerb::View),
                    owner: subsumption::owner(granted),
                    recipient: RecipientPattern::parse("*").unwrap(),
                    resource: AgentResourcePattern::Any,
                });
                let check = PermissionTarget::Agent(ClassPermissionTarget {
                    owner: subsumption::owner(requested),
                    verb: Some(AgentVerb::View),
                    resource: AgentResourcePattern::Any,
                });
                let view = single(CatalogOperation::ViewAgent, Resource::None);
                let (today, decided) = answers(&surface_of(grant, &holder()), &view, check);
                assert_eq!(today, covers);
                assert_eq!(decided, today);
            }
        );
    }
}

/// The recipient matching cases of the subsumption tests: an `invoke` grant reaches its holder
/// exactly where its recipient covers the holder, today and in the decision.
#[test_gen]
fn generate_decision_over_recipient_matching_tests(r: &mut DynamicTestRegistration) {
    for (recipient, matches) in subsumption::recipient_matching_cases() {
        add_test!(
            r,
            format!(
                "decision_over_recipient_matching_{}",
                subsumption::test_name(recipient)
            ),
            TestProperties::unit_test(),
            move || {
                let grant = PermissionPattern::Agent(ClassPermissionPattern {
                    verb: Some(AgentVerb::Invoke),
                    owner: AgentOwnerPattern::parse(&format!("{OWNER}/*")).unwrap(),
                    recipient: subsumption::parse_recipient(recipient),
                    resource: AgentResourcePattern::Any,
                });
                let holder = subsumption::parse_recipient(subsumption::RECIPIENT_HOLDER);
                let (today, decided) = answers(
                    &surface_of(grant, &holder),
                    &invoke_increment(),
                    check("agent", "invoke", "increment"),
                );
                assert_eq!(today, matches);
                assert_eq!(decided, today);
            }
        );
    }
}

/// The path glob cases of the subsumption tests: a file read is allowed exactly where the path
/// of the grant covers the path of the request, today and in the decision.
#[test_gen]
fn generate_decision_over_glob_resource_tests(r: &mut DynamicTestRegistration) {
    for (index, (name, ..)) in subsumption::glob_resource_cases().into_iter().enumerate() {
        add_test!(
            r,
            format!("decision_over_glob_resource_{name}"),
            TestProperties::unit_test(),
            move || {
                let (_, granted, requested, covers) =
                    subsumption::glob_resource_cases().swap_remove(index);
                let owner = "acme/shop/prod/cart/00000007-0000-8000-8000-000000000002";
                let grant = subsumption::fs(owner, "acme/*/*/*/*", granted);
                let holder = subsumption::parse_recipient(subsumption::RECIPIENT_HOLDER);
                let (today, decided) = answers(
                    &surface_of(grant, &holder),
                    &read_file(),
                    subsumption::fs_target(owner, requested),
                );
                assert_eq!(today, covers);
                assert_eq!(decided, today);
            }
        );
    }
}

/// The type wildcard cases of the subsumption tests: a grant on `Cart(*)` covers every kind of
/// `Cart` agent and no other agent, today and in the decision. No REST agent operation checks
/// the `env` and `config` classes, so they have no decision here.
#[test]
fn a_type_wildcard_grant_decides_as_today_for_every_agent_of_the_type() {
    let holder = RecipientPattern::parse("acme/shop/prod/cart/Caller").unwrap();
    let untyped = AgentOwnerLeafPattern::agent(AgentId::from_parts(7, 1), None);
    for (class, verb, resource) in subsumption::AGENT_OWNED_CLASSES {
        let operation = match class {
            "agent" => single(
                CatalogOperation::InvokeAgent,
                Resource::Method(resource.to_string()),
            ),
            "filesystem" => single(
                CatalogOperation::ReadFile,
                Resource::Path(resource.to_string()),
            ),
            "oplog" => read_oplog(),
            _ => continue,
        };
        let grant = |owner: &str| {
            parse_permission(&format!(
                "{class}(acme/shop/prod/cart/{owner}) @ * : {verb} : {resource}"
            ))
            .unwrap()
        };
        let request = |leaf: &AgentOwnerLeafPattern| {
            subsumption::with_agent_leaf(
                grant("00000007-0000-8000-8000-000000000001").to_target(),
                leaf,
            )
        };
        let surface = surface_of(grant("Cart(*)"), &holder);
        let agents = subsumption::agents_of_type("Cart", 7)
            .into_iter()
            .map(|(kind, agent)| (kind, agent, true))
            .chain(
                subsumption::agents_of_type("CartX", 8)
                    .into_iter()
                    .map(|(kind, agent)| (kind, agent, false)),
            )
            .chain([("untyped", untyped.clone(), false)]);
        for (kind, agent, covers) in agents {
            let (today, decided) = answers(&surface, &operation, request(&agent));
            assert_eq!(today, covers, "{class}: {kind}");
            assert_eq!(decided, today, "{class}: {kind}");
        }
    }
}

#[test]
fn agent_exec_is_allowed_exactly_where_invoke_star_is() {
    let grants: [&[&str]; 4] = [
        &["agent(acme/shop/prod/cart/*) @ acme : invoke : *"],
        &["agent(acme/shop/prod/cart/*) @ acme : invoke : increment"],
        &["agent(acme/shop/prod/cart/*) @ acme : view : *"],
        &["agent(acme/shop/prod/billing/*) @ acme : invoke : *"],
    ];
    for lower in grants {
        let surface = surface(lower, &[], &[]);
        let invoke_star = surface.authorize(&check("agent", "invoke", "*")).unwrap();
        let exec = exec_check(agent_owner());
        assert_eq!(exec, check("agent", "invoke", "*"), "{lower:?}");
        let decision = decide_with_checks(&surface, &hosted_exec(), &[vec![exec], vec![]]);
        assert_eq!(
            is_allowed(&decision),
            invoke_star,
            "{lower:?}: {decision:?}"
        );
    }
}

#[test]
fn a_hosted_exec_needs_both_parts() {
    let tool_check = grant("tool(acme/shop/prod/*/*) @ acme : invoke : *").to_target();
    let agent_only = surface(
        &["agent(acme/shop/prod/cart/*) @ acme : invoke : *"],
        &[],
        &[],
    );
    assert_eq!(
        decide_with_checks(
            &agent_only,
            &hosted_exec(),
            &[vec![exec_check(agent_owner())], vec![tool_check.clone()]]
        ),
        Decision::Deny(Denial::Forbidden {
            missing_grant: None
        })
    );

    let tool_only = surface(&["tool(acme/shop/prod/*/*) @ acme : invoke : *"], &[], &[]);
    assert_eq!(
        decide_with_checks(
            &tool_only,
            &hosted_exec(),
            &[vec![exec_check(agent_owner())], vec![tool_check]]
        ),
        Decision::Deny(Denial::LooksUnknown {
            kind: DesignatedKind::Agent
        })
    );
}

#[test]
fn a_denied_agent_part_looks_like_an_unknown_agent() {
    let surface = surface(&[], &[], &[]);
    assert_eq!(
        decide_with_checks(
            &surface,
            &invoke_increment(),
            &[vec![check("agent", "invoke", "increment")]]
        ),
        Decision::Deny(Denial::LooksUnknown {
            kind: DesignatedKind::Agent
        })
    );
}

#[test]
fn every_check_of_a_part_must_allow() {
    let list_files = single(
        CatalogOperation::ListFiles,
        Resource::Path("/data".to_string()),
    );
    let checks = || {
        vec![vec![
            check("filesystem", "stat", "/data"),
            check("filesystem", "list", "/data"),
        ]]
    };
    let stat_only = surface(
        &["filesystem(acme/shop/prod/cart/*) @ acme : stat : /**"],
        &[],
        &[],
    );
    assert!(!is_allowed(&decide_with_checks(
        &stat_only,
        &list_files,
        &checks()
    )));

    let both = surface(
        &[
            "filesystem(acme/shop/prod/cart/*) @ acme : stat : /**",
            "filesystem(acme/shop/prod/cart/*) @ acme : list : /**",
        ],
        &[],
        &[],
    );
    assert!(is_allowed(&decide_with_checks(
        &both,
        &list_files,
        &checks()
    )));
}

#[test]
fn a_check_that_does_not_belong_to_the_part_is_refused() {
    let surface = surface(&["agent(acme/shop/prod/cart/*) @ acme : * : *"], &[], &[]);
    // The surface allows `delete`, but `delete` is not how 1.6 checks an invoke.
    assert!(surface.authorize(&check("agent", "delete", "")).unwrap());
    assert!(!is_allowed(&decide_with_checks(
        &surface,
        &invoke_increment(),
        &[vec![check("agent", "delete", "")]]
    )));
    assert!(!check_belongs_to_part(
        v2::Verb::Agent(v2::AgentVerb::Invoke),
        &check("agent", "delete", "")
    ));
    assert!(check_belongs_to_part(
        v2::Verb::Agent(v2::AgentVerb::Invoke),
        &check("agent", "invoke", "increment")
    ));
}

#[test]
fn a_missing_part_check_list_denies() {
    let surface = surface(&["agent(acme/shop/prod/cart/*) @ acme : * : *"], &[], &[]);
    assert!(!is_allowed(&decide_with_checks(
        &surface,
        &hosted_exec(),
        &[vec![exec_check(agent_owner())]]
    )));
}

#[test]
fn an_allow_names_the_card_whose_grant_allowed() {
    let view = card_with(&["agent(acme/shop/prod/cart/*) @ acme : view : *"]);
    let invoke = card_with(&["agent(acme/shop/prod/cart/*) @ acme : invoke : *"]);
    let surface = EffectiveSurface::from_cards(&[view, invoke.clone()], &holder()).unwrap();
    assert_eq!(
        decide_with_checks(
            &surface,
            &invoke_increment(),
            &[vec![check("agent", "invoke", "increment")]]
        ),
        Decision::Allow(Grantors {
            cards: vec![GrantorCard {
                card_id: invoke.card_id,
                ancestors: Vec::new(),
                presented_by: None,
            }]
        })
    );
}

#[test]
fn an_invocation_read_is_answered_by_todays_agent_view() {
    let read = v2::Verb::Invocation(v2::InvocationVerb::Read);
    assert!(check_belongs_to_part(read, &check("agent", "view", "*")));
    assert!(!check_belongs_to_part(read, &check("agent", "invoke", "*")));
}
