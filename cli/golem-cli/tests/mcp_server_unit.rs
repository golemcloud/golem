// Copyright 2024-2025 Golem Cloud
//
// Unit tests for MCP server types (DTOs, request/response serde).

use golem_cli::service::mcp_server::{
    GetComponentRequest, ListAgentTypesResponse, ListComponentsResponse, ListWorkersResponse,
    McpComponentDto, McpWorkerDto,
};

#[test]
fn unit_list_agent_types_response_serde() {
    let res = ListAgentTypesResponse {
        agent_types: vec!["AgentA".to_string(), "AgentB".to_string()],
    };
    let json = serde_json::to_value(&res).unwrap();
    let back: ListAgentTypesResponse = serde_json::from_value(json).unwrap();
    assert_eq!(back.agent_types, res.agent_types);
}

#[test]
fn unit_list_agent_types_response_empty() {
    let res = ListAgentTypesResponse {
        agent_types: vec![],
    };
    let json = serde_json::to_value(&res).unwrap();
    let back: ListAgentTypesResponse = serde_json::from_value(json).unwrap();
    assert!(back.agent_types.is_empty());
}

#[test]
fn unit_mcp_component_dto_serde() {
    let dto = McpComponentDto {
        id: "id-1".to_string(),
        name: "comp1".to_string(),
        revision: 1,
        size: 100,
    };
    let json = serde_json::to_value(&dto).unwrap();
    let back: McpComponentDto = serde_json::from_value(json).unwrap();
    assert_eq!(back.id, dto.id);
    assert_eq!(back.name, dto.name);
    assert_eq!(back.revision, dto.revision);
    assert_eq!(back.size, dto.size);
}

#[test]
fn unit_list_components_response_serde() {
    let res = ListComponentsResponse {
        components: vec![
            McpComponentDto {
                id: "a".to_string(),
                name: "n1".to_string(),
                revision: 1,
                size: 10,
            },
            McpComponentDto {
                id: "b".to_string(),
                name: "n2".to_string(),
                revision: 2,
                size: 20,
            },
        ],
    };
    let json = serde_json::to_value(&res).unwrap();
    let back: ListComponentsResponse = serde_json::from_value(json).unwrap();
    assert_eq!(back.components.len(), 2);
    assert_eq!(back.components[0].name, "n1");
    assert_eq!(back.components[1].name, "n2");
}

#[test]
fn unit_list_workers_response_serde() {
    let res = ListWorkersResponse {
        workers: vec![McpWorkerDto {
            worker_id: "w1".to_string(),
            component_name: "c1".to_string(),
            component_id: "cid1".to_string(),
            status: "Running".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            last_error: None,
            retry_count: 0,
            pending_invocation_count: 0,
        }],
    };
    let json = serde_json::to_value(&res).unwrap();
    let back: ListWorkersResponse = serde_json::from_value(json).unwrap();
    assert_eq!(back.workers.len(), 1);
    assert_eq!(back.workers[0].worker_id, "w1");
    assert_eq!(back.workers[0].component_name, "c1");
}

#[test]
fn unit_get_component_request_serde() {
    let req = GetComponentRequest {
        component_name: "my-component".to_string(),
        revision: Some(2),
    };
    let json = serde_json::to_value(&req).unwrap();
    let back: GetComponentRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back.component_name, "my-component");
    assert_eq!(back.revision, Some(2));
}

#[test]
fn unit_get_component_request_no_revision() {
    let req = GetComponentRequest {
        component_name: "x".to_string(),
        revision: None,
    };
    let json = serde_json::to_value(&req).unwrap();
    let back: GetComponentRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back.component_name, "x");
    assert_eq!(back.revision, None);
}
