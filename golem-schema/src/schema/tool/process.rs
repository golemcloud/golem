// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

//! Output policy shared by shell and child-process adapters for bound tools.
//!
//! A named [`ProcessExitStatus`] controls the process status. Other structured
//! results become redacted text only when the command has no stdout stream.

use super::ProcessExitStatus;
use crate::schema::render::json_value::to_json_value_redacted;
use crate::{IntoSchema, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
use std::collections::BTreeMap;

/// Validate the cross-language named process-status contract.
pub fn process_status_type(graph: &SchemaGraph, root: &SchemaType) -> Result<bool, String> {
    if !matches!(root, SchemaType::Ref { id, .. } if *id == ProcessExitStatus::type_id()) {
        return Ok(false);
    }
    if let Some(definition) = graph.lookup(&ProcessExitStatus::type_id())
        && let SchemaType::Record { fields, .. } = &definition.body
        && fields.len() == 1
        && fields[0].name == "exit_code"
        && matches!(
            fields[0].body,
            SchemaType::U8 {
                restrictions: None,
                ..
            }
        )
    {
        return Ok(true);
    }
    Err("ProcessExitStatus must define exactly one exit_code: u8 field".into())
}

/// A structured result's contribution, separate from streamed output.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProcessResult {
    pub exit_code: u8,
    /// `None` preserves the command's streamed stdout, including an empty stream.
    pub stdout: Option<Vec<u8>>,
}

/// Render an already decoded result without revealing opaque capabilities.
///
/// The caller must retain an earlier stream failure or cancellation status in
/// preference to this status. A decoding failure is a command failure, not a
/// spawn failure.
pub fn render_result(
    stdout: bool,
    result_is_status: bool,
    value: Option<&TypedSchemaValue>,
) -> Result<ProcessResult, String> {
    if stdout && !result_is_status {
        return Ok(ProcessResult::default());
    }
    let Some(typed) = value else {
        return if result_is_status {
            Err("result: missing ProcessExitStatus".into())
        } else {
            Ok(ProcessResult::default())
        };
    };
    if result_is_status {
        if !process_status_type(typed.graph(), typed.root_type())? {
            return Err("result: expected named ProcessExitStatus".into());
        }
        let SchemaValue::Record { fields } = typed.value() else {
            return Err("result: expected ProcessExitStatus record".into());
        };
        let [SchemaValue::U8(exit_code)] = fields.as_slice() else {
            return Err("result: expected exactly one u8 exit status value".into());
        };
        return Ok(ProcessResult {
            exit_code: *exit_code,
            stdout: None,
        });
    }
    let json = to_json_value_redacted(typed.graph(), typed.root_type(), typed.value())
        .map_err(|error| format!("result: {error:?}"))?;
    let stdout = match json {
        serde_json::Value::String(text) => text.into_bytes(),
        value => format!("{value}\n").into_bytes(),
    };
    Ok(ProcessResult {
        exit_code: 0,
        stdout: Some(stdout),
    })
}

/// A declared tool error uses its metadata status, or one when undeclared.
/// Invalid or unavailable payloads never fall back to a capability debug dump.
pub fn render_custom_error(
    name: &str,
    payload: Option<&TypedSchemaValue>,
    declared: &BTreeMap<String, u8>,
) -> (u8, String) {
    let payload = payload.and_then(|typed| {
        to_json_value_redacted(typed.graph(), typed.root_type(), typed.value()).ok()
    });
    let message = match payload {
        Some(payload) => format!("tool error: {name}: {payload}"),
        None => format!("tool error: {name}"),
    };
    (declared.get(name).copied().unwrap_or(1), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IntoTypedSchemaValue, try_into_schema_graph};
    use test_r::test;

    #[test]
    fn process_results_preserve_streams_and_named_status() {
        for exit_code in [0, 7, 255] {
            let value = ProcessExitStatus { exit_code }
                .into_typed_schema_value()
                .unwrap();
            for stdout in [false, true] {
                assert_eq!(
                    render_result(stdout, true, Some(&value)).unwrap(),
                    ProcessResult {
                        exit_code,
                        stdout: None
                    }
                );
            }
        }
        let value = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String("text".into()),
        );
        assert_eq!(
            render_result(true, false, Some(&value)).unwrap(),
            ProcessResult::default()
        );
        assert_eq!(
            render_result(false, false, Some(&value)).unwrap().stdout,
            Some(b"text".to_vec())
        );
        assert!(render_result(true, true, Some(&value)).is_err());
        assert!(render_result(false, true, None).is_err());
        assert_eq!(
            render_result(false, false, None).unwrap(),
            ProcessResult::default()
        );
    }

    #[test]
    fn process_status_rejects_a_structurally_similar_unnamed_record() {
        let graph = try_into_schema_graph::<ProcessExitStatus>().unwrap();
        assert!(process_status_type(&graph, &graph.root).unwrap());
        let unnamed = graph.resolve_ref(&graph.root).unwrap().clone();
        assert!(!process_status_type(&graph, &unnamed).unwrap());
        let mut malformed = graph.clone();
        let definition = malformed
            .defs
            .iter_mut()
            .find(|def| def.id == ProcessExitStatus::type_id())
            .unwrap();
        let SchemaType::Record { fields, .. } = &mut definition.body else {
            panic!()
        };
        fields[0].name = "code".into();
        assert!(process_status_type(&malformed, &malformed.root).is_err());
    }

    #[test]
    fn custom_process_errors_use_declared_status_without_debug_fallback() {
        let declared = BTreeMap::from([("invalid".into(), 2)]);
        let payload = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::string()),
            SchemaValue::String("bad value".into()),
        );
        assert_eq!(
            render_custom_error("invalid", Some(&payload), &declared),
            (2, "tool error: invalid: \"bad value\"".into())
        );
        assert_eq!(
            render_custom_error("unknown", None, &declared),
            (1, "tool error: unknown".into())
        );
    }

    #[cfg(not(all(feature = "guest", not(feature = "host"))))]
    #[test]
    fn process_results_and_errors_redact_nested_capabilities() {
        use crate::schema::schema_type::{NamedFieldType, SecretSpec};
        use crate::schema::schema_value::SecretValuePayload;
        let value = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(vec![NamedFieldType {
                name: "credential".into(),
                body: SchemaType::secret(SecretSpec::default()),
                metadata: Default::default(),
            }])),
            SchemaValue::Record {
                fields: vec![SchemaValue::Secret(SecretValuePayload {
                    secret_id: uuid::Uuid::nil(),
                    config_key: Some(vec!["private-key".into()]),
                    version: 0,
                    resolved_at: chrono::DateTime::UNIX_EPOCH,
                    category: None,
                })],
            },
        );
        assert_eq!(
            render_result(false, false, Some(&value)).unwrap().stdout,
            Some(b"{\"credential\":\"<redacted: secret>\"}\n".to_vec())
        );
        assert_eq!(
            render_custom_error("failure", Some(&value), &BTreeMap::new()),
            (
                1,
                "tool error: failure: {\"credential\":\"<redacted: secret>\"}".into()
            )
        );
    }
}
