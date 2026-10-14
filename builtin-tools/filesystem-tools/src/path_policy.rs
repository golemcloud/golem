use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, SchemaValue, TypedSchemaValue,
    universal_tool_middleware,
};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Filesystem operation classes understood by the path policy middleware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoSchema, FromSchema)]
#[schema(rename_all = "snake_case")]
pub enum PathPolicyOperation {
    Read,
    Write,
    Delete,
}

/// Operations permitted below one owner-filesystem root.
#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub struct PathPolicyRoot {
    /// Absolute owner-filesystem path, or a path relative to `base`.
    pub path: String,
    pub operations: Vec<PathPolicyOperation>,
}

/// Installation parameters for the built-in `path-policy` middleware.
#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub struct PathPolicyParameters {
    /// Absolute owner-filesystem directory used to resolve relative invocation paths and roots.
    pub base: String,
    pub allowed_roots: Vec<PathPolicyRoot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProtectedPathArgument {
    pub(crate) operation: PathPolicyOperation,
    pub(crate) argument: &'static str,
}

pub(crate) fn protected_path_argument(
    tool_name: &str,
    command_path: &[String],
) -> Result<Option<ProtectedPathArgument>, Vec<String>> {
    let operation = match tool_name {
        "read-file" | "ls" | "grep" => PathPolicyOperation::Read,
        "write-file" | "edit-file" => PathPolicyOperation::Write,
        "delete-file" => PathPolicyOperation::Delete,
        _ => return Ok(None),
    };
    if !command_path.is_empty() {
        return Err(command_path.to_vec());
    }
    Ok(Some(ProtectedPathArgument {
        operation,
        argument: "path",
    }))
}

pub(crate) fn validate_path_policy_parameters(
    parameters: &PathPolicyParameters,
) -> Result<(), String> {
    let base = Path::new(&parameters.base);
    if parameters.base.is_empty()
        || parameters.base.contains('\0')
        || parameters.base.contains('\\')
    {
        return Err(
            "path-policy base must be a nonempty path without NUL bytes or backslashes".to_string(),
        );
    }
    if !base.is_absolute() {
        return Err("path-policy base must be an absolute owner-filesystem path".to_string());
    }
    if parameters.allowed_roots.is_empty() {
        return Err("path-policy must declare at least one allowed root".to_string());
    }
    for root in &parameters.allowed_roots {
        if root.path.is_empty() || root.path.contains('\0') || root.path.contains('\\') {
            return Err(
                "path-policy allowed roots must be nonempty paths without NUL bytes or backslashes"
                    .to_string(),
            );
        }
        if root.operations.is_empty() {
            return Err(format!(
                "path-policy root '{}' must allow at least one operation",
                root.path
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathPolicyViolation {
    pub(crate) supplied_path: String,
    pub(crate) resolved_path: Option<PathBuf>,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema)]
pub(crate) struct PathPolicyDenied {
    pub(crate) operation: PathPolicyOperation,
    pub(crate) argument: String,
    pub(crate) supplied_path: String,
    pub(crate) resolved_path: Option<String>,
    pub(crate) allowed_roots: Vec<String>,
    pub(crate) reason: String,
}

pub(crate) fn authorize_path(
    parameters: &PathPolicyParameters,
    operation: PathPolicyOperation,
    supplied_path: &str,
) -> Result<PathBuf, PathPolicyViolation> {
    validate_path_policy_parameters(parameters).map_err(|reason| PathPolicyViolation {
        supplied_path: supplied_path.to_string(),
        resolved_path: None,
        reason,
    })?;
    if supplied_path.is_empty() || supplied_path.contains('\0') || supplied_path.contains('\\') {
        return Err(PathPolicyViolation {
            supplied_path: supplied_path.to_string(),
            resolved_path: None,
            reason: "path must be nonempty and contain no NUL bytes or backslashes".to_string(),
        });
    }

    let base =
        normalize_owner_path(Path::new("/"), Path::new(&parameters.base)).map_err(|reason| {
            PathPolicyViolation {
                supplied_path: supplied_path.to_string(),
                resolved_path: None,
                reason,
            }
        })?;
    let resolved = normalize_owner_path(&base, Path::new(supplied_path)).map_err(|reason| {
        PathPolicyViolation {
            supplied_path: supplied_path.to_string(),
            resolved_path: None,
            reason,
        }
    })?;

    let allowed = parameters.allowed_roots.iter().any(|root| {
        root.operations.contains(&operation)
            && normalize_owner_path(&base, Path::new(&root.path))
                .is_ok_and(|allowed_root| resolved.starts_with(allowed_root))
    });
    if !allowed {
        return Err(PathPolicyViolation {
            supplied_path: supplied_path.to_string(),
            resolved_path: Some(resolved),
            reason: "resolved path is outside every root allowed for this operation".to_string(),
        });
    }

    reject_symlink_components(&resolved).map_err(|reason| PathPolicyViolation {
        supplied_path: supplied_path.to_string(),
        resolved_path: Some(resolved.clone()),
        reason,
    })?;
    Ok(resolved)
}

fn normalize_owner_path(base: &Path, path: &Path) -> Result<PathBuf, String> {
    let mut normalized = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        base.to_path_buf()
    };
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("path traverses above the owner-filesystem root".to_string());
                }
            }
            Component::Prefix(_) => {
                return Err("path uses an unsupported platform prefix".to_string());
            }
        }
    }
    if !normalized.is_absolute() {
        return Err("resolved owner-filesystem path is not absolute".to_string());
    }
    Ok(normalized)
}

fn reject_symlink_components(path: &Path) -> Result<(), String> {
    fs::symlink_metadata("/")
        .map_err(|error| format!("failed to inspect owner-filesystem root '/': {error}"))?;
    let mut current = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(component) => current.push(component),
            Component::CurDir => continue,
            Component::ParentDir | Component::Prefix(_) => {
                return Err("resolved path contains an invalid component".to_string());
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "resolved path traverses symbolic link '{}'",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(format!(
                    "failed to inspect owner-filesystem path '{}': {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn rewrite_authorized_path(
    parameters: &PathPolicyParameters,
    protected: ProtectedPathArgument,
    tool_metadata: &Tool,
    command_path: &[String],
    input: TypedSchemaValue,
) -> Result<TypedSchemaValue, ToolInvokeError<RawCustomToolError>> {
    let command_index = tool_metadata
        .command_index_by_path(command_path)
        .ok_or_else(|| ToolInvokeError::InvalidCommandPath(command_path.to_vec()))?;
    let (graph, value) = input.into_parts();
    let mut fields = tool_metadata
        .decode_canonical_input_record(command_index, value)
        .map_err(|error| ToolInvokeError::InvalidInput(error.to_string()))?;
    let path = fields
        .iter_mut()
        .find(|field| field.name == protected.argument)
        .ok_or_else(|| {
            ToolInvokeError::InvalidInput(format!(
                "protected tool input has no '{}' field",
                protected.argument
            ))
        })?;
    let SchemaValue::String(supplied_path) = &path.value else {
        return Err(ToolInvokeError::InvalidInput(format!(
            "protected tool input field '{}' is not a string",
            protected.argument
        )));
    };
    let resolved = match authorize_path(parameters, protected.operation, supplied_path) {
        Ok(resolved) => resolved,
        Err(violation) => {
            let payload = PathPolicyDenied {
                operation: protected.operation,
                argument: protected.argument.to_string(),
                supplied_path: violation.supplied_path,
                resolved_path: violation
                    .resolved_path
                    .map(|path| path.to_string_lossy().into_owned()),
                allowed_roots: parameters
                    .allowed_roots
                    .iter()
                    .filter(|root| root.operations.contains(&protected.operation))
                    .map(|root| root.path.clone())
                    .collect(),
                reason: violation.reason,
            }
            .into_typed_schema_value()
            .map_err(|error| ToolInvokeError::InternalError(error.to_string()))?;
            return Err(ToolInvokeError::UnknownCustomError(
                RawCustomToolError::from_payload("path-policy-denied".to_string(), payload),
            ));
        }
    };
    path.value = SchemaValue::String(resolved.to_string_lossy().into_owned());
    Ok(TypedSchemaValue::new(
        graph,
        SchemaValue::Record {
            fields: fields.into_iter().map(|field| field.value).collect(),
        },
    ))
}

pub(crate) fn apply_path_policy(
    parameters: &PathPolicyParameters,
    tool_name: &str,
    tool_metadata: &Tool,
    command_path: &[String],
    input: TypedSchemaValue,
) -> Result<TypedSchemaValue, ToolInvokeError<RawCustomToolError>> {
    match protected_path_argument(tool_name, command_path)
        .map_err(ToolInvokeError::InvalidCommandPath)?
    {
        Some(protected) => {
            rewrite_authorized_path(parameters, protected, tool_metadata, command_path, input)
        }
        None => Ok(input),
    }
}

/// Restricts the built-in filesystem tools to operator-configured owner-filesystem roots.
#[universal_tool_middleware(
    name = "path-policy",
    version = "0.1.1",
    parameters = PathPolicyParameters
)]
async fn path_policy(
    parameters: PathPolicyParameters,
    tool_name: String,
    tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    _principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let input = apply_path_policy(
        &parameters,
        &tool_name,
        &tool_metadata,
        &command_path,
        input,
    )?;
    underlying
        .invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_is_restricted_to_read_enabled_roots() {
        let protected = protected_path_argument("grep", &[])
            .unwrap()
            .expect("grep must be protected");
        assert_eq!(protected.operation, PathPolicyOperation::Read);

        let parameters = PathPolicyParameters {
            base: "/tmp".to_string(),
            allowed_roots: vec![PathPolicyRoot {
                path: "project".to_string(),
                operations: vec![PathPolicyOperation::Read],
            }],
        };
        assert_eq!(
            authorize_path(&parameters, protected.operation, "project/src").unwrap(),
            Path::new("/tmp/project/src")
        );
        assert!(authorize_path(&parameters, protected.operation, "other/src").is_err());
    }
}
