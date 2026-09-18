use super::*;
use golem_common::base_model::UntypedJsonBody;
use golem_common::base_model::retry_policy::{ApiNeverPolicy, ApiPredicateFalse};
use golem_common::base_model::retry_policy::{ApiPredicate, ApiRetryPolicy};
use golem_common::model::Empty;
use golem_common::model::agent::CorsOptions;
use golem_common::model::agent_secret::{AgentSecretId, AgentSecretPath};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceDefinitionId, ResourceLimit, ResourceName,
};
use golem_common::model::retry_policy::{RetryPolicyId, RetryPolicyRevision};
use golem_common::schema::agent::{InputSchema, NamedField, OutputSchema};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::{ExternalSchemaValue, SchemaGraph, SchemaTypeDef, SchemaValue, TypeId};
use test_r::test;
use uuid::Uuid;

fn schema_str() -> SchemaType {
    SchemaType::string()
}

fn http_method(input: SchemaType, output: OutputSchema) -> AgentMethodSchema {
    AgentMethodSchema {
        name: "exchange".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::parameters([NamedField::user_supplied("input", input)]),
        output_schema: output,
        http_endpoint: vec![HttpEndpointDetails {
            http_method: HttpMethod::Post(Empty {}),
            path_suffix: vec![],
            header_vars: vec![],
            query_vars: vec![],
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
        }],
        read_only: None,
    }
}

fn route_mode_diff(graph: &SchemaGraph, stream_method: &AgentMethodSchema) -> String {
    let rest_method = http_method(SchemaType::string(), OutputSchema::Unit);
    let current = display_method(&SourceLanguage::TypeScript, graph, &rest_method);
    let new = display_method(&SourceLanguage::TypeScript, graph, stream_method);

    diff::unified_diff(
        serde_yaml::to_string(&current).unwrap(),
        serde_yaml::to_string(&new).unwrap(),
    )
}

#[test]
fn deploy_diff_http_route_mode_tracks_method_streams() {
    let empty_graph = SchemaGraph::empty();
    let rest = display_method(
        &SourceLanguage::TypeScript,
        &empty_graph,
        &http_method(SchemaType::string(), OutputSchema::Unit),
    );
    assert_eq!(rest.http[0].route_mode, "Rest");

    let input_stream = http_method(
        SchemaType::stream(Some(SchemaType::string())),
        OutputSchema::Unit,
    );
    let output_stream = http_method(
        SchemaType::string(),
        OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8())))),
    );
    let ref_graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("StreamRef"),
            name: None,
            body: SchemaType::stream(Some(SchemaType::u64())),
        }],
        root: SchemaType::record(vec![]),
    };
    let referenced_stream = http_method(
        SchemaType::ref_to(TypeId::new("StreamRef")),
        OutputSchema::Unit,
    );

    for (graph, method) in [
        (&empty_graph, &input_stream),
        (&empty_graph, &output_stream),
        (&ref_graph, &referenced_stream),
    ] {
        let rendered = display_method(&SourceLanguage::TypeScript, graph, method);
        assert_eq!(rendered.http[0].route_mode, "DurableStreams");

        let route_mode_change = route_mode_diff(graph, method);
        assert!(route_mode_change.contains("-  routeMode: Rest"));
        assert!(route_mode_change.contains("+  routeMode: DurableStreams"));
    }
}

fn secret_dto(
    path: &[&str],
    secret_type: SchemaGraph,
    value: Option<SchemaValue>,
) -> AgentSecretDto {
    AgentSecretDto {
        id: AgentSecretId(Uuid::nil()),
        environment_id: EnvironmentId(Uuid::nil()),
        path: CanonicalAgentSecretPath::from_path_in_unknown_casing(
            &path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ),
        revision: serde_json::from_value(serde_json::json!(0)).unwrap(),
        secret_type,
        secret_value: value.map(|value| ExternalSchemaValue::try_from(value).unwrap()),
    }
}

fn retry_policy(name: &str, priority: u32) -> RetryPolicyDto {
    RetryPolicyDto {
        id: RetryPolicyId(Uuid::nil()),
        environment_id: EnvironmentId(Uuid::nil()),
        name: name.to_string(),
        revision: RetryPolicyRevision::INITIAL,
        priority,
        predicate: UntypedJsonBody(
            serde_json::to_value(ApiPredicate::False(ApiPredicateFalse {})).unwrap(),
        ),
        policy: UntypedJsonBody(
            serde_json::to_value(ApiRetryPolicy::Never(ApiNeverPolicy {})).unwrap(),
        ),
    }
}

fn resource(name: &str, limit_value: u64) -> ResourceDefinition {
    ResourceDefinition {
        id: ResourceDefinitionId(Uuid::nil()),
        revision: serde_json::from_value(serde_json::json!(0)).unwrap(),
        environment_id: EnvironmentId(Uuid::nil()),
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: limit_value }),
        enforcement_action: EnforcementAction::Reject,
        unit: "unit".to_string(),
        units: "units".to_string(),
    }
}

fn resource_creation(name: &str, limit_value: u64) -> ResourceDefinitionCreation {
    ResourceDefinitionCreation {
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: limit_value }),
        enforcement_action: EnforcementAction::Reject,
        unit: "unit".to_string(),
        units: "units".to_string(),
    }
}

#[test]
fn tool_publication_plan_distinguishes_work_and_conflicts() {
    let entry = |action| ToolPublicationPlanEntry {
        action,
        name: "example".to_string(),
        version: "1.0.0".to_string(),
        reason: None,
    };

    assert!(
        !ToolPublicationPlan::new(
            BTreeSet::new(),
            vec![entry(ToolPublicationPlanAction::NoChange)]
        )
        .has_work()
    );
    assert!(
        ToolPublicationPlan::new(
            BTreeSet::new(),
            vec![entry(ToolPublicationPlanAction::Publish)]
        )
        .has_work()
    );
    assert!(
        ToolPublicationPlan::new(
            BTreeSet::new(),
            vec![entry(ToolPublicationPlanAction::Conflict)]
        )
        .has_conflicts()
    );
}

#[test]
fn environment_setup_secret_type_rendering_matches_between_manifest_and_environment() {
    let mut secret_types = BTreeMap::new();
    secret_types.insert("superSecret".to_string(), schema_str());

    let plan = build_environment_setup_plan(
        MaskingConfig::hide_secrets(),
        vec![DeploymentAgentSecretDefault {
            path: AgentSecretPath(vec!["superSecret".to_string()]),
            secret_value: serde_json::json!("same-value"),
        }],
        Vec::new(),
        Vec::new(),
        vec![secret_dto(
            &["superSecret"],
            SchemaGraph::anonymous(SchemaType::string()),
            Some(SchemaValue::String("same-value".to_string())),
        )],
        Vec::new(),
        Vec::new(),
        &secret_types,
        &SourceLanguage::TypeScript,
    )
    .unwrap();

    assert!(
        plan.display
            .skipped_already_exists
            .secret_values
            .contains("superSecret")
    );
}

#[test]
fn environment_setup_classifies_secret_create_and_skip_existing() {
    let mut secret_types = BTreeMap::new();
    secret_types.insert("createSecret".to_string(), schema_str());
    secret_types.insert("existingSecret".to_string(), schema_str());

    let plan = build_environment_setup_plan(
        MaskingConfig::hide_secrets(),
        vec![
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["createSecret".to_string()]),
                secret_value: serde_json::json!("create"),
            },
            DeploymentAgentSecretDefault {
                path: AgentSecretPath(vec!["existingSecret".to_string()]),
                secret_value: serde_json::json!("manifest"),
            },
        ],
        Vec::new(),
        Vec::new(),
        vec![secret_dto(
            &["existingSecret"],
            SchemaGraph::anonymous(SchemaType::string()),
            Some(SchemaValue::String("env".to_string())),
        )],
        Vec::new(),
        Vec::new(),
        &secret_types,
        &SourceLanguage::TypeScript,
    )
    .unwrap();

    assert!(
        plan.display
            .to_be_applied
            .secret_values
            .contains_key("createSecret")
    );
    assert!(
        plan.display
            .skipped_already_exists
            .secret_values
            .contains("existingSecret")
    );
}

#[test]
fn environment_setup_classifies_retry_policies_and_resources() {
    let plan = build_environment_setup_plan(
        MaskingConfig::hide_secrets(),
        Vec::new(),
        vec![
            DeploymentRetryPolicyDefault {
                name: "create-policy".to_string(),
                priority: 1,
                predicate: ApiPredicate::False(ApiPredicateFalse {}),
                policy: ApiRetryPolicy::Never(ApiNeverPolicy {}),
            },
            DeploymentRetryPolicyDefault {
                name: "existing-policy".to_string(),
                priority: 2,
                predicate: ApiPredicate::False(ApiPredicateFalse {}),
                policy: ApiRetryPolicy::Never(ApiNeverPolicy {}),
            },
        ],
        vec![
            resource_creation("create-resource", 1),
            resource_creation("existing-resource", 2),
        ],
        Vec::new(),
        vec![retry_policy("existing-policy", 999)],
        vec![resource("existing-resource", 999)],
        &BTreeMap::new(),
        &SourceLanguage::TypeScript,
    )
    .unwrap();

    assert!(
        plan.display
            .to_be_applied
            .retry_policies
            .contains_key("create-policy")
    );
    assert!(
        plan.display
            .skipped_already_exists
            .retry_policies
            .contains("existing-policy")
    );

    assert!(
        plan.display
            .to_be_applied
            .resources
            .contains_key("create-resource")
    );
    assert!(
        plan.display
            .skipped_already_exists
            .resources
            .contains("existing-resource")
    );
}
