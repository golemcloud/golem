// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use super::*;
use crate::http::{is_managed_response_header, valid_decoded_segment};
use crate::schema::SchemaType;
use crate::schema::tool::OptionSpec;
use crate::schema::tool::canonical::{CanonicalSurfaceRef, EffectiveCommandField};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy)]
enum Source {
    Path,
    Query,
    Header,
}

impl Tool {
    /// Checks HTTP declarations independently of CLI/schema validation, so
    /// descriptor producers can use the same rules as canonical ingestion.
    pub fn validate_http(&self) -> Result<(), ToolValidationError> {
        let routes = self.http_endpoints()?;
        self.validate_http_prefixes(0, "")?;
        let mut identities = HashSet::new();
        let mut route_ids = HashSet::new();
        for route in routes {
            let command = route.command_path.join(".");
            let fail = |input: Option<&str>, detail: &str| http_error(&command, input, detail);
            if route.route_id.is_empty() {
                return Err(fail(
                    None,
                    "HTTP route ID must not be empty; a root endpoint requires an explicit route ID",
                ));
            }
            if !route_ids.insert(route.route_id) {
                return Err(fail(None, "duplicate HTTP route ID"));
            }
            let endpoint = route.endpoint;
            let method = method_name(&endpoint.http_method);
            if http::Method::from_bytes(method.as_bytes()).is_err() {
                return Err(fail(None, "invalid HTTP method"));
            }
            let mut identity = Vec::new();
            for (index, segment) in route.path.iter().enumerate() {
                match segment {
                    PathSegment::Literal(literal) => {
                        if !valid_decoded_segment(&literal.value) {
                            return Err(fail(None, "invalid literal path segment"));
                        }
                        identity.push((0, literal.value.clone()));
                    }
                    PathSegment::PathVariable(_) => identity.push((1, String::new())),
                    PathSegment::RemainingPathVariable(_) => {
                        if index + 1 != route.path.len() {
                            return Err(fail(None, "remaining path capture must be last"));
                        }
                        identity.push((2, String::new()));
                    }
                }
            }
            if !identities.insert((method, identity)) {
                return Err(fail(None, "duplicate method and equivalent path template"));
            }
            let inputs: BTreeMap<_, _> = self
                .canonical_input_surfaces(route.command_index)
                .into_iter()
                .filter_map(|surface| {
                    self.canonical_field_for_surface(route.command_index, surface)
                        .map(|field| (field.name.clone(), (field, surface)))
                })
                .collect();
            let mut bound = HashSet::new();
            let mut bind = |name: &str, source| {
                let (field, surface) = inputs
                    .get(name)
                    .ok_or_else(|| fail(Some(name), "binding names an unknown input"))?;
                if matches!(source, Source::Path)
                    && matches!(
                        surface,
                        CanonicalSurfaceRef::GlobalFlag { .. }
                            | CanonicalSurfaceRef::BodyFlag { .. }
                    )
                {
                    return Err(fail(Some(name), "a flag cannot bind from the HTTP path"));
                }
                if !bound.insert(name.to_string()) {
                    return Err(fail(Some(name), "input has more than one HTTP binding"));
                }
                if !self.http_binding_type(&field.type_, source) {
                    return Err(fail(
                        Some(name),
                        "input shape cannot bind from this HTTP source",
                    ));
                }
                Ok(())
            };
            for segment in &route.path {
                if let PathSegment::PathVariable(var) | PathSegment::RemainingPathVariable(var) =
                    segment
                {
                    bind(&var.variable_name, Source::Path)?;
                }
            }
            let mut query_names = HashSet::new();
            for var in &endpoint.query_vars {
                if var.query_param_name.is_empty() || !query_names.insert(&var.query_param_name) {
                    return Err(fail(
                        Some(&var.variable_name),
                        "empty or duplicate query parameter",
                    ));
                }
                bind(&var.variable_name, Source::Query)?;
            }
            let mut header_names = HashSet::new();
            for var in &endpoint.header_vars {
                let name = http::HeaderName::from_bytes(var.header_name.as_bytes())
                    .map_err(|_| fail(Some(&var.variable_name), "invalid input header name"))?;
                if !header_names.insert(name) {
                    return Err(fail(
                        Some(&var.variable_name),
                        "duplicate input header name",
                    ));
                }
                bind(&var.variable_name, Source::Header)?;
            }
            if matches!(method, "GET" | "HEAD" | "DELETE") {
                for (name, (field, _)) in &inputs {
                    if !bound.contains(name) {
                        if !query_names.insert(name) {
                            return Err(fail(
                                Some(name),
                                "implicit query name conflicts with a query template",
                            ));
                        }
                        if !self.http_binding_type(&field.type_, Source::Query) {
                            return Err(fail(
                                Some(name),
                                "input shape cannot bind from implicit query",
                            ));
                        }
                        bound.insert(name.clone());
                    }
                }
            }
            let body = self.commands.nodes[route.command_index]
                .body
                .as_ref()
                .unwrap();
            if route.mode == HttpRouteMode::Plain && body.stdin.is_some() {
                for (name, (_, surface)) in &inputs {
                    if !bound.contains(name)
                        && self.http_input_required(route.command_index, *surface)
                    {
                        return Err(fail(
                            Some(name),
                            "plain stdin leaves a required input in the JSON body",
                        ));
                    }
                }
            }
            let mut names = HashSet::new();
            for header in &endpoint.response_headers {
                let name = http::HeaderName::from_bytes(header.name.as_bytes())
                    .map_err(|_| fail(None, "invalid response header name"))?;
                if is_managed_response_header(name.as_str()) || !names.insert(name) {
                    return Err(fail(None, "managed or duplicate response header"));
                }
                http::HeaderValue::from_bytes(header.value.as_bytes())
                    .map_err(|_| fail(None, "invalid response header value"))?;
            }
        }
        Ok(())
    }

    fn validate_http_prefixes(
        &self,
        index: usize,
        command: &str,
    ) -> Result<(), ToolValidationError> {
        let node = &self.commands.nodes[index];
        let check = |path: &[PathSegment]| {
            let globals = self.effective_globals(index);
            for segment in path {
                match segment {
                    PathSegment::Literal(literal) if !valid_decoded_segment(&literal.value) => {
                        return Err(http_error(command, None, "invalid literal path segment"));
                    }
                    PathSegment::PathVariable(var) | PathSegment::RemainingPathVariable(var) => {
                        let ty = globals.iter().find_map(|global| match global {
                            EffectiveCommandField::Option(option)
                                if option.long == var.variable_name =>
                            {
                                Some(super::super::canonical::option_collected_type(
                                    &option.shape,
                                ))
                            }
                            _ => None,
                        });
                        if !ty
                            .as_ref()
                            .is_some_and(|ty| self.http_binding_type(ty, Source::Path))
                        {
                            return Err(http_error(
                                command,
                                Some(&var.variable_name),
                                "mount/prefix capture must name a scalar global declared here or above",
                            ));
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        };
        if index == 0
            && let Some(mount) = &self.http_mount
        {
            check(&mount.path_prefix)?;
        }
        if let Some(CommandHttp::Prefix(prefix)) = &node.http {
            check(&prefix.path_suffix)?;
        }
        // An endpoint also supplies its suffix to all descendants.
        if !node.subcommands.is_empty()
            && let Some(CommandHttp::Endpoint(endpoint)) = &node.http
        {
            check(&endpoint.path_suffix)?;
        }
        for child in &node.subcommands {
            let index = child.0 as usize;
            let name = &self.commands.nodes[index].name;
            let path = if command.is_empty() {
                name.clone()
            } else {
                format!("{command}.{name}")
            };
            self.validate_http_prefixes(index, &path)?;
        }
        Ok(())
    }

    fn http_binding_type<'a>(&'a self, mut ty: &'a SchemaType, source: Source) -> bool {
        let mut visited = HashSet::new();
        loop {
            let Ok(resolved) = self.schema.resolve_ref(ty) else {
                return false;
            };
            if !visited.insert(resolved as *const SchemaType) {
                return false;
            }
            ty = resolved;
            match ty {
                SchemaType::Option { inner, .. } => ty = inner,
                _ => break,
            }
        }
        match ty {
            SchemaType::List { element, .. } | SchemaType::FixedList { element, .. }
                if !matches!(source, Source::Path) =>
            {
                self.http_binding_type(element, Source::Path)
            }
            SchemaType::Map { key, value, .. } if matches!(source, Source::Query) => {
                self.http_binding_type(key, Source::Path)
                    && self.http_binding_type(value, Source::Path)
            }
            SchemaType::Bool { .. }
            | SchemaType::S8 { .. }
            | SchemaType::S16 { .. }
            | SchemaType::S32 { .. }
            | SchemaType::S64 { .. }
            | SchemaType::U8 { .. }
            | SchemaType::U16 { .. }
            | SchemaType::U32 { .. }
            | SchemaType::U64 { .. }
            | SchemaType::F32 { .. }
            | SchemaType::F64 { .. }
            | SchemaType::Char { .. }
            | SchemaType::String { .. }
            | SchemaType::Enum { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Uuid { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. } => true,
            _ => false,
        }
    }

    fn http_input_required(&self, command: usize, surface: CanonicalSurfaceRef) -> bool {
        let body = self.commands.nodes[command].body.as_ref().unwrap();
        let required = |option: &OptionSpec| option.required && option.default.is_none();
        match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => {
                required(&self.commands.nodes[node].globals.options[index])
            }
            CanonicalSurfaceRef::BodyOption { index } => required(&body.options[index]),
            CanonicalSurfaceRef::BodyPositional { index } => {
                let positional = &body.positionals.fixed[index];
                positional.required && positional.default.is_none()
            }
            CanonicalSurfaceRef::BodyTail => body
                .positionals
                .tail
                .as_ref()
                .is_some_and(|tail| tail.min > 0),
            CanonicalSurfaceRef::GlobalFlag { .. } | CanonicalSurfaceRef::BodyFlag { .. } => false,
        }
    }
}

fn http_error(command: &str, input: Option<&str>, detail: &str) -> ToolValidationError {
    ToolValidationError::InvalidHttpDeclaration {
        command: command.into(),
        input: input.map(str::to_string),
        detail: detail.into(),
    }
}

fn method_name(method: &HttpMethod) -> &str {
    match method {
        HttpMethod::Get(_) => "GET",
        HttpMethod::Head(_) => "HEAD",
        HttpMethod::Post(_) => "POST",
        HttpMethod::Put(_) => "PUT",
        HttpMethod::Delete(_) => "DELETE",
        HttpMethod::Connect(_) => "CONNECT",
        HttpMethod::Options(_) => "OPTIONS",
        HttpMethod::Trace(_) => "TRACE",
        HttpMethod::Patch(_) => "PATCH",
        HttpMethod::Custom(method) => &method.value,
    }
}
