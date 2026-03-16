// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::printer::*;
use crate::rust::lib_gen::{Module, ModuleDef, ModuleName};
use crate::rust::model_gen::RefCache;
use crate::rust::printer::*;
use crate::rust::types::{
    ref_or_box_schema_type, ref_or_schema_type, DataType, ModelType, RustPrinter, RustResult,
};
use crate::{Error, Result};
use convert_case::{Case, Casing};
use indexmap::IndexMap;
use itertools::Itertools;
use openapiv3::{
    OpenAPI, Operation, Parameter, ParameterData, ParameterSchemaOrContent, PathItem, ReferenceOr,
    RequestBody, Response, Schema, SchemaKind, StatusCode, Tag, Type,
};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathElement {
    Const(String),
    Param { name: String },
}

impl PathElement {
    pub fn from_string(p: &str) -> PathElement {
        if p.starts_with('{') && p.ends_with('}') {
            PathElement::Param {
                name: p
                    .strip_prefix('{')
                    .unwrap()
                    .strip_suffix('}')
                    .unwrap()
                    .to_string(),
            }
        } else {
            PathElement::Const(p.to_string())
        }
    }

    pub fn name(&self) -> String {
        match self {
            PathElement::Const(name) => name.to_string(),
            PathElement::Param { name } => name.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct Path(Vec<PathElement>);

impl Path {
    pub fn common_prefix(&self, that: Path) -> Path {
        if self.0.starts_with(&that.0) {
            return that;
        }

        let elems: Vec<PathElement> = self
            .0
            .iter()
            .zip(that.0)
            .take_while(|(a, b)| a == &b)
            .map(|(_, e)| e)
            .collect();

        Path(elems)
    }

    pub fn from_string(path: &str) -> Path {
        let elems: Vec<PathElement> = path
            .split('/')
            .filter(|p| !p.trim().is_empty())
            .map(PathElement::from_string)
            .collect();

        Path(elems)
    }

    pub fn strip_prefix(&self, prefix_len: usize) -> Path {
        let slice = &self.0[prefix_len..];
        Path(slice.to_vec())
    }

    pub fn method_name(&self, method: &str) -> String {
        #[allow(unstable_name_collisions)]
        let name: String = self
            .0
            .iter()
            .map(|e| e.name().to_case(Case::Snake))
            .intersperse("_".to_string())
            .collect();

        if name.is_empty() {
            method.to_string()
        } else {
            format!("{name}_{method}")
        }
    }
}

struct PathOperation {
    path: Path,
    original_path: String,
    method: String,
    op: Operation,
}

#[derive(Debug, Clone)]
struct Method {
    name: String,
    path: Path,
    original_path: String,
    http_method: String,
    params: Vec<Param>,
    result: DataType,
    result_status_code: StatusCode,
    errors: MethodErrors,
}

fn error_name(method_name: &str, error_kind: &ErrorKind) -> String {
    match error_kind {
        ErrorKind::Common { name } => name.clone(),
        ErrorKind::Custom { prefix } => {
            format!("{prefix}{}Error", method_name.to_case(Case::UpperCamel))
        }
    }
}

fn client_error() -> RustPrinter {
    rust_name("crate", "Error")
}

impl Method {
    fn render_declaration(&self, error_kind: &ErrorKind) -> RustPrinter {
        let params = self
            .params
            .iter()
            .map(|p| unit() + ", " + p.render_declaration())
            .reduce(|acc, e| acc + e)
            .unwrap_or_else(unit);

        let result = self.result.render_declaration(false);

        #[rustfmt::skip]
        let code = unit() + "async fn " + &self.name + "(&self" + params + ") -> Result<" + result + ", " + client_error() + "<" + error_name(&self.name, error_kind) + ">>";

        code
    }

    fn get_path_param(&self, original_name: &str) -> Option<&Param> {
        self.params
            .iter()
            .find(|p| p.kind == ParamKind::Path && p.original_name == original_name)
    }
}

enum ErrorKind {
    Common { name: String },
    Custom { prefix: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MethodErrors {
    codes: BTreeMap<StatusCode, DataType>,
}

#[derive(Debug, Clone)]
struct Param {
    original_name: String,
    name: String,
    tpe: DataType,
    required: bool,
    kind: ParamKind,
}

#[derive(Debug, Clone)]
pub struct RequestBodyParams {
    params: HashMap<ContentType, Vec<Param>>,
}

impl RequestBodyParams {
    fn has_single_content_type(&self) -> bool {
        self.params.len() == 1
    }

    fn get_default_request_body_param(&self) -> Option<&Vec<Param>> {
        self.params
            .values()
            .next()
            .filter(|_| self.has_single_content_type())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ContentType(pub String);

impl ContentType {
    pub fn is_json(&self) -> bool {
        self.0 == "application/json"
    }

    pub fn is_yaml(&self) -> bool {
        self.0 == "application/x-yaml"
    }
}

impl Param {
    fn render_declaration(&self) -> RustPrinter {
        let type_name = self.tpe.render_declaration(true);

        let type_name = if self.required {
            type_name
        } else {
            unit() + "Option<" + type_name + ">"
        };

        unit() + &self.name + ": " + type_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParamKind {
    Path,
    Query,
    Header,
    Cookie,
    Body,
    Multipart,
}

impl ParamKind {
    fn from_parameter(param: &Parameter) -> ParamKind {
        match param {
            Parameter::Query { .. } => ParamKind::Query,
            Parameter::Header { .. } => ParamKind::Header,
            Parameter::Path { .. } => ParamKind::Path,
            Parameter::Cookie { .. } => ParamKind::Cookie,
        }
    }
}

fn tag_operations(
    tag: &Option<Tag>,
    path: &str,
    path_item: &ReferenceOr<PathItem>,
) -> Vec<PathOperation> {
    if let Some(item) = path_item.as_item() {
        if let Some(tag) = tag {
            item.iter()
                .filter(|(_, op)| op.tags.contains(&tag.name))
                .map(|(method, op)| PathOperation {
                    path: Path::from_string(path),
                    original_path: path.to_string(),
                    method: method.to_string(),
                    op: op.clone(),
                })
                .collect()
        } else {
            item.iter()
                .filter(|(_, op)| op.tags.is_empty())
                .map(|(method, op)| PathOperation {
                    path: Path::from_string(path),
                    original_path: path.to_string(),
                    method: method.to_string(),
                    op: op.clone(),
                })
                .collect()
        }
    } else {
        Vec::new()
    }
}

fn match_tag(tag: &Option<Tag>, path_item: &ReferenceOr<PathItem>) -> bool {
    if let Some(item) = path_item.as_item() {
        if let Some(tag) = tag {
            item.iter().any(|(_, op)| op.tags.contains(&tag.name))
        } else {
            item.iter().any(|(_, op)| op.tags.is_empty())
        }
    } else {
        false
    }
}

fn param_data_to_type(data: &ParameterData, ref_cache: &mut RefCache) -> Result<DataType> {
    match &data.format {
        ParameterSchemaOrContent::Schema(ref_or_schema) => {
            ref_or_schema_type(ref_or_schema, ref_cache, None)
        }
        ParameterSchemaOrContent::Content(_) => {
            Err(Error::unimplemented("Content parameter is not supported."))
        }
    }
}

fn parameter(p: &ReferenceOr<Parameter>, ref_cache: &mut RefCache) -> Result<Param> {
    if let Some(param) = p.as_item() {
        let param_type = param_data_to_type(param.parameter_data_ref(), ref_cache)?;

        Ok(Param {
            original_name: param.parameter_data_ref().name.to_string(),
            name: param.parameter_data_ref().name.to_case(Case::Snake),
            tpe: param_type,
            required: param.parameter_data_ref().required,
            kind: ParamKind::from_parameter(param),
        })
    } else {
        Err(Error::unimplemented("Unexpected ref parameter."))
    }
}

fn request_body_params(
    body: &ReferenceOr<RequestBody>,
    ref_cache: &mut RefCache,
) -> Result<RequestBodyParams> {
    let mut content_type_params = HashMap::new();

    match body {
        ReferenceOr::Reference { reference } => {
            return Err(Error::unimplemented(format!(
                "Unexpected ref request body: '{reference}'."
            )))
        }
        ReferenceOr::Item(body) => {
            for (content_type, media_type) in &body.content {
                if content_type.starts_with("application/json") || content_type == "*/*" {
                    let schema = match &media_type.schema {
                        None => Err(Error::unimplemented("JSON content without schema.")),
                        Some(schema) => Ok(schema),
                    };

                    content_type_params.insert(
                        ContentType(content_type.clone()),
                        vec![Param {
                            original_name: "".to_string(),
                            name: "value".to_string(),
                            tpe: ref_or_schema_type(
                                schema?,
                                ref_cache,
                                Some(content_type.clone()),
                            )?,
                            required: body.required,
                            kind: ParamKind::Body,
                        }],
                    );
                } else if content_type == "application/octet-stream" {
                    content_type_params.insert(
                        ContentType(content_type.clone()),
                        vec![Param {
                            original_name: "".to_string(),
                            name: "value".to_string(),
                            tpe: DataType::Binary,
                            required: body.required,
                            kind: ParamKind::Body,
                        }],
                    );
                } else if content_type.contains("application/x-yaml") {
                    let schema = match &media_type.schema {
                        None => Err(Error::unimplemented("YAML content without schema.")),
                        Some(schema) => Ok(schema),
                    };

                    let param = Param {
                        original_name: "".to_string(),
                        name: "value".to_string(),
                        tpe: ref_or_schema_type(schema?, ref_cache, Some(content_type.clone()))?,
                        required: body.required,
                        kind: ParamKind::Body,
                    };

                    content_type_params.insert(ContentType(content_type.clone()), vec![param]);
                } else if content_type == "multipart/form-data" {
                    match &media_type.schema {
                        None => {
                            return Err(Error::unimplemented("Multipart content without schema."))
                        }
                        Some(schema) => match schema {
                            ReferenceOr::Reference { reference } => {
                                return Err(Error::unimplemented(format!(
                                    "Unexpected ref multipart schema: '{reference}'."
                                )))
                            }
                            ReferenceOr::Item(schema) => match &schema.schema_kind {
                                SchemaKind::Type(Type::Object(obj)) => {
                                    fn multipart_param(
                                        name: &str,
                                        required: bool,
                                        schema: &ReferenceOr<Box<Schema>>,
                                        ref_cache: &mut RefCache,
                                    ) -> Result<Param> {
                                        Ok(Param {
                                            original_name: name.to_string(),
                                            name: name.to_case(Case::Snake),
                                            tpe: ref_or_box_schema_type(schema, ref_cache)?,
                                            required,
                                            kind: ParamKind::Multipart,
                                        })
                                    }

                                    let params = obj
                                        .properties
                                        .iter()
                                        .map(|(name, schema)| {
                                            multipart_param(
                                                name,
                                                body.required && obj.required.contains(name),
                                                schema,
                                                ref_cache,
                                            )
                                        })
                                        .collect::<Result<Vec<_>>>()?;

                                    content_type_params
                                        .insert(ContentType(content_type.clone()), params);
                                }
                                _ => {
                                    return Err(Error::unimplemented(
                                        "Object schema expected for multipart request body.",
                                    ))
                                }
                            },
                        },
                    }
                } else {
                    return Err(Error::unimplemented(format!(
                        "Request body content type: '{content_type}'."
                    )));
                }
            }
        }
    }

    Ok(RequestBodyParams {
        params: content_type_params,
    })
}

fn parameters(op: &PathOperation, ref_cache: &mut RefCache) -> Result<Vec<Param>> {
    op.op
        .parameters
        .iter()
        .map(|p| parameter(p, ref_cache))
        .collect()
}

fn as_code(code: &StatusCode) -> Option<u16> {
    match code {
        StatusCode::Code(code) => Some(*code),
        StatusCode::Range(_) => None,
    }
}

fn as_range(code: &StatusCode) -> Option<u16> {
    match code {
        StatusCode::Code(_) => None,
        StatusCode::Range(v) => Some(*v),
    }
}

fn is_success_code(code: &StatusCode) -> bool {
    if let Some(code) = as_code(code) {
        (200..300).contains(&code)
    } else {
        false
    }
}

fn is_success_range(code: &StatusCode) -> bool {
    if let Some(v) = as_range(code) {
        v == 2
    } else {
        false
    }
}

fn response_type(response: &ReferenceOr<Response>, ref_cache: &mut RefCache) -> Result<DataType> {
    match response {
        ReferenceOr::Reference { reference } => Err(Error::unimplemented(format!(
            "Reference in response top level: {reference}"
        ))),
        ReferenceOr::Item(resp) => {
            if resp.content.is_empty() {
                // No content case
                Ok(DataType::Unit)
            } else if resp.content.len() == 1 {
                let (content_type, media_type) = resp.content.first().unwrap();

                if content_type.starts_with("application/json") {
                    let schema = match &media_type.schema {
                        None => Err(Error::unimplemented(
                            "JSON response content without schema.",
                        )),
                        Some(schema) => Ok(schema),
                    };

                    Ok(ref_or_schema_type(
                        schema?,
                        ref_cache,
                        Some(content_type.clone()),
                    )?)
                } else if content_type == "application/octet-stream" {
                    Ok(DataType::Binary)
                } else {
                    Err(Error::unimplemented(format!(
                        "Response content type: {content_type}"
                    )))
                }
            } else {
                Err(Error::unimplemented(
                    "Response content with not exactly 1 option.",
                ))
            }
        }
    }
}

fn method_result(
    responses: &IndexMap<StatusCode, ReferenceOr<Response>>,
    ref_cache: &mut RefCache,
) -> Result<(StatusCode, DataType)> {
    let code_res = responses
        .iter()
        .filter(|(code, _)| is_success_code(code))
        .min_by_key(|(code, _)| as_code(code));

    let res = code_res.or_else(|| responses.iter().find(|(code, _)| is_success_range(code)));

    if let Some((code, resp)) = res {
        Ok((code.clone(), response_type(resp, ref_cache)?))
    } else {
        Err(Error::unimplemented("No success results in API."))
    }
}

fn method_errors(
    responses: &IndexMap<StatusCode, ReferenceOr<Response>>,
    result_code: StatusCode,
    ref_cache: &mut RefCache,
) -> Result<MethodErrors> {
    let codes: Result<BTreeMap<StatusCode, DataType>> = responses
        .iter()
        .filter(|(code, _)| *code != &result_code)
        .map(|(code, resp)| response_type(resp, ref_cache).map(|t| (code.clone(), t)))
        .collect();

    Ok(MethodErrors { codes: codes? })
}

fn trait_methods_specific_to_content_type(
    op: &PathOperation,
    prefix_length: usize,
    ref_cache: &mut RefCache,
) -> Result<Vec<Method>> {
    let (result_code, result_type) = method_result(&op.op.responses.responses, ref_cache)?;

    let name = op
        .op
        .operation_id
        .as_ref()
        .map(|op_id| op_id.to_case(Case::Snake))
        .unwrap_or_else(|| op.path.strip_prefix(prefix_length).method_name(&op.method));

    let mut main_params = parameters(op, ref_cache)?;

    if let Some(body) = &op.op.request_body {
        let content_specific = request_body_params(body, ref_cache)?;

        if let Some(request_body_params) = content_specific.get_default_request_body_param() {
            main_params.extend(request_body_params.iter().cloned());
            return Ok(vec![create_method(
                name,
                op,
                &main_params,
                result_type,
                result_code,
                ref_cache,
            )?]);
        }

        let mut methods = Vec::new();
        for (content_type, params) in content_specific.params {
            let method_name = match_content_type(content_type, &name)?;
            let new_params = [main_params.clone(), params].concat();
            methods.push(create_method(
                method_name,
                op,
                &new_params,
                result_type.clone(),
                result_code.clone(),
                ref_cache,
            )?);
        }

        Ok(methods)
    } else {
        Ok(vec![create_method(
            name,
            op,
            &main_params,
            result_type,
            result_code,
            ref_cache,
        )?])
    }
}

fn create_method(
    name: String,
    op: &PathOperation,
    params: &[Param],
    result_type: DataType,
    result_code: StatusCode,
    ref_cache: &mut RefCache,
) -> Result<Method> {
    Ok(Method {
        name,
        path: op.path.clone(),
        original_path: op.original_path.clone(),
        http_method: op.method.to_string(),
        params: params.to_vec(),
        result: result_type,
        result_status_code: result_code.clone(),
        errors: method_errors(&op.op.responses.responses, result_code, ref_cache)?,
    })
}

fn match_content_type(content_type: ContentType, base_name: &str) -> Result<String> {
    if content_type.is_json() {
        Ok(format!("{}_json", base_name))
    } else if content_type.is_yaml() {
        Ok(format!("{}_yaml", base_name))
    } else {
        Err(Error::unimplemented(
            "Multiple content types supported only for JSON and YAML",
        ))
    }
}

fn trait_methods(
    operations: &[PathOperation],
    prefix_length: usize,
    ref_cache: &mut RefCache,
) -> Result<Vec<Method>> {
    let res = operations
        .iter()
        .map(|op| trait_methods_specific_to_content_type(op, prefix_length, ref_cache))
        .collect::<Result<Vec<Vec<_>>>>()?;

    Ok(res.into_iter().flatten().collect())
}

fn render_errors(method_name: &str, error_kind: &ErrorKind, errors: &MethodErrors) -> RustResult {
    let name = error_name(method_name, error_kind);

    let code_cases = errors
        .codes
        .iter()
        .map(|(code, model)| {
            line(unit() + "Error" + code.to_string() + "(" + model.render_declaration(false) + "),")
        })
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    let display_cases = errors
        .codes
        .iter()
        .map(|(code, model)| {
            line(
                unit()
                    + name.clone()
                    + "::Error"
                    + code.to_string()
                    + "(body) => write!(f, \"{}\", "
                    + render_error_body_to_string(model)
                    + "),",
            )
        })
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    #[rustfmt::skip]
    let res = unit() +
        line(unit() + "#[derive(Debug)]") +
        line(unit() + "pub enum " + name.clone() + " {") +
        indented(
            code_cases
        ) +
        line(unit() + "}") +
        line(unit() + "impl std::fmt::Display for " + name.clone() + " {") +
        indented(
            line(unit() + "fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {") +
                indented(
                    if !errors.codes.is_empty() {
                         line(unit() + "match self {") +
                         indented(display_cases) +
                         line(unit() + "}")
                       } else { line(unit() + indented(unit() + "write!(f, \"" + name + "\")")) }
                ) +
                line("}")
        ) +
        line("}");

    Ok(res)
}

fn render_error_body_to_string(typ: &DataType) -> RustPrinter {
    match typ {
        DataType::Model(ModelType { name }) if name == "ErrorBody" => unit() + r#"&body.error"#,
        DataType::Model(ModelType { name }) if name == "ErrorsBody" => {
            unit() + r#"body.errors.clone().join(", ")"#
        }
        DataType::Model(ModelType { name }) if name == "WorkerServiceErrorsBody" => {
            unit()
                + "match &body { WorkerServiceErrorsBody::Messages("
                + rust_name("crate::model", "ErrorsBody")
                + " { errors }) => { errors.join(\", \") }, WorkerServiceErrorsBody::Validation("
                + rust_name("crate::model", "ErrorsBody")
                + " { errors }) => { errors.join(\", \") }}"
        }
        DataType::Model(ModelType { name }) if name == "GolemErrorBody" => {
            unit() + r#"format!("{}", body.golem_error)"#
        }
        _ => unit() + r#"format!("{body:?}")"#,
    }
}

fn async_annotation() -> RustPrinter {
    unit() + "#[" + rust_name("async_trait", "async_trait") + "]"
}

fn render_path_param(method: &Method, name: &str) -> RustResult {
    match method.get_path_param(name) {
        None => Err(Error::unexpected(format!(
            "Can't find path parameter {name}"
        ))),
        Some(param) => match &param.tpe {
            DataType::String => Ok(unit() + &param.name),
            DataType::Uuid => Ok(unit() + "&" + &param.name + ".to_string()"),
            DataType::Model(_) => Ok(unit() + "&" + &param.name + ".to_string()"),
            DataType::Int(_) => Ok(unit() + "&" + &param.name + ".to_string()"),
            _ => Err(Error::unexpected(format!(
                "Unexpected param type {name}: {:?}",
                param.tpe
            ))),
        },
    }
}

fn render_path_element(method: &Method, e: &PathElement) -> RustResult {
    match e {
        PathElement::Const(segment) => Ok(indent() + r#".push(""# + segment + r#"")"#),
        PathElement::Param { name } => {
            Ok(indent() + ".push(" + render_path_param(method, name)? + ")")
        }
    }
}

#[rustfmt::skip]
fn unwrap_optional_param(name: &str, setter: RustPrinter) -> RustPrinter {
    line(unit() + "if let Some(" + name + ") = " + name + " {") +
        indented(
            setter
        ) +
        line("}")
}

#[rustfmt::skip]
fn for_param(name: &str, setter: RustPrinter) -> RustPrinter {
    line(unit() + "for " + name + " in " + name + " {") +
        indented(
            setter
        ) +
        line("}")
}

fn param_to_str(param: &Param) -> RustResult {
    param_tpe_to_str(&param.original_name, &param.name, &param.tpe)
}

fn param_tpe_to_str(original_name: &str, name: &str, tpe: &DataType) -> RustResult {
    match tpe {
        DataType::String => Ok(unit() + name),
        DataType::Uuid => Ok(unit() + name + ".to_string()"),
        DataType::Int(_) => Ok(unit() + name + ".to_string()"),
        DataType::Boolean => Ok(unit() + name + ".to_string()"),
        DataType::Model(_) => Ok(unit() + name + ".to_string()"),
        _ => Err(Error::unexpected(format!(
            "Unexpected query param type {}: {:?}",
            original_name, tpe
        ))),
    }
}

fn query_setter(param: &Param) -> RustResult {
    let code = match &param.tpe {
        DataType::Array(tpe) => {
            #[rustfmt::skip] let setter =
                    line(unit() + r#"url.query_pairs_mut().append_pair(""# + &param.original_name + r#"", &"# + param_tpe_to_str(&param.original_name, &param.name, tpe)? + ");");

            for_param(&param.name, setter)
        }
        _ => {
            #[rustfmt::skip] let setter =
                    line(unit() + r#"url.query_pairs_mut().append_pair(""# + &param.original_name + r#"", &"# + param_to_str(param)? + ");");
            setter
        }
    };

    let code = if param.required {
        code
    } else {
        unwrap_optional_param(&param.name, code)
    };

    Ok(code)
}

fn header_setter(param: &Param) -> RustResult {
    #[rustfmt::skip]
    let code =
        line(unit() + r#"headers.append(""# + &param.original_name + r#"", "# + rust_name("reqwest::header", "HeaderValue") + "::from_str(&" + param_to_str(param)? + ")?);");

    let code = if param.required {
        code
    } else {
        unwrap_optional_param(&param.name, code)
    };

    Ok(code)
}

#[rustfmt::skip]
fn make_part(param: &Param) -> RustResult {
    let part_type = rust_name("reqwest::multipart", "Part");

    let inner =
        if param.tpe == DataType::Binary {
            Ok(indent() + r#"form = form.part(""# + &param.original_name + r#"", "# + part_type + "::stream(" + &param.name + r#").mime_str("application/octet-stream")?);"#)
        } else if param.tpe == DataType::String {
            Ok(indent() + r#"form = form.part(""# + &param.original_name + r#"", "# + part_type + "::text(" + &param.name + r#".to_string()).mime_str("text/plain; charset=utf-8")?);"#)
        }
        else if let DataType::Model(_) = param.tpe {
            Ok(indent() + r#"form = form.part(""# + &param.original_name + r#"", "# + part_type + "::text(crate::model::MultipartField::to_multipart_field(" + &param.name + r#")).mime_str(crate::model::MultipartField::mime_type("# + &param.name + r#"))?);"#)
        } else {
            Err(Error::unimplemented(format!("Unsupported multipart part type {:?}", param.tpe)))
        };

    if param.required {
        inner
    } else {
        Ok(
            indent() + line(unit() + r#"if let Some("# + &param.name + r#") = "# + &param.name + " {") +
                indented(inner?) +
            line("}")
        )
    }
}

fn status_match(range_results: bool, code: &StatusCode) -> RustPrinter {
    match code {
        StatusCode::Code(code) => {
            if range_results {
                unit() + "(" + code.to_string() + ", _)"
            } else {
                unit() + code.to_string()
            }
        }
        StatusCode::Range(range) => unit() + "(_, " + range.to_string() + ")",
    }
}

fn response_body_parsing(data_type: &DataType) -> RustPrinter {
    match data_type {
        DataType::Binary => unit() + "response.bytes().await?",
        DataType::Unit => unit() + "()",
        _ => unit() + "response.json::<" + data_type.render_declaration(false) + ">().await?",
    }
}

fn error_case(
    range_results: bool,
    code: &StatusCode,
    data_type: &DataType,
    method_name: &str,
    error_kind: &ErrorKind,
) -> RustPrinter {
    #[rustfmt::skip]
    let res = unit() +
        line(status_match(range_results, code) + " => {") +
        indented(
            line(unit() + "let body = " + response_body_parsing(data_type) + ";") +
            line(unit() + "Err(" + client_error() + "::Item(" + error_name(method_name, error_kind) + "::Error" + code.to_string() + "(body)))")
        ) +
        line("}");

    res
}

fn render_method_implementation(method: &Method, error_kind: &ErrorKind) -> RustResult {
    let path_segments: Result<Vec<RustPrinter>> = method
        .path
        .0
        .iter()
        .map(|s| render_path_element(method, s))
        .collect();

    #[rustfmt::skip]
    let url = unit() +
        line("let mut url = self.context.base_url.clone();") +
        indent() + "url.path_segments_mut().unwrap()" +
        indented(
           path_segments?.into_iter().map(|s| unit() + NewLine + s).reduce(|acc, e| acc + e). unwrap_or_else(unit) + ";" + NewLine
        );

    let query_setters: Result<Vec<RustPrinter>> = method
        .params
        .iter()
        .filter(|p| p.kind == ParamKind::Query)
        .map(query_setter)
        .collect();

    if method.params.iter().any(|p| p.kind == ParamKind::Cookie) {
        return Err(Error::unimplemented("Cookie parameters."));
    }

    let header_setters: Result<Vec<RustPrinter>> = method
        .params
        .iter()
        .filter(|p| p.kind == ParamKind::Header)
        .map(header_setter)
        .collect();

    let header_setters = header_setters?;

    let no_headers = header_setters.is_empty();

    let headers_vec = if no_headers {
        unit()
    } else {
        line(
            r#"let headers_vec: Vec<(&str, String)> = headers.iter().map(|(k, v)| (k.as_str(), format!("{:?}", v))).collect();"#,
        )
    };

    let headers_log = if no_headers {
        unit()
    } else {
        unit() + ", headers=?headers_vec"
    };

    let method_log = unit() + r#"method=""# + &method.http_method + r#"""#;

    let body_param = method.params.iter().find(|p| p.kind == ParamKind::Body);
    let is_multipart = method.params.iter().any(|p| p.kind == ParamKind::Multipart);

    let body_log = if let Some(body_param) = body_param {
        if body_param.tpe == DataType::Binary {
            unit() + r#", body="<binary>""#
        } else if let DataType::Model(_) = &body_param.tpe {
            unit() + ", body=serde_json::to_string(" + &body_param.name + ")?"
        } else {
            unit() + ", body=?" + &body_param.name
        }
    } else if is_multipart {
        unit() + r#", body="<multipart>""#
    } else {
        unit()
    };

    let endpoint_log = unit() + r#", endpoint=""# + &method.original_path + r#"""#;

    let logs = headers_vec
        + line(
            unit()
                + "tracing::info!("
                + method_log
                + endpoint_log
                + ", url=url.to_string()"
                + headers_log
                + body_log
                + r#", ""#
                + &method.name
                + r#"");"#,
        );

    #[rustfmt::skip]
    let headers_and_logs =
        if no_headers {
            logs
        } else {
            unit() +
                line(unit() + "let mut headers = " + rust_name("reqwest::header", "HeaderMap") + "::new();") +
                NewLine +
                header_setters.into_iter().map(|l| l + NewLine).reduce(|acc, e| acc + e). unwrap_or_else(unit) +
                NewLine +
                line("{") +
                indented(logs) +
                line("}") +
                NewLine +
                line("request = request.headers(headers);") +
                NewLine
        };

    let body_setter = match body_param {
        None => unit(),
        Some(param) => {
            if param.tpe == DataType::Binary {
                line(unit() + "request = request.body(" + &param.name + ");")
                    + line(
                        r#"request = request.header(reqwest::header::CONTENT_TYPE, "application/octet-stream");"#,
                    )
                    + NewLine
            } else if param.tpe == DataType::Yaml {
                line(
                    unit()
                        + "request = request.body(serde_yaml::to_string("
                        + &param.name
                        + ").unwrap_or_default().into_bytes());",
                ) + line(
                    r#"request = request.header(reqwest::header::CONTENT_TYPE, "application/x-yaml");"#,
                ) + NewLine
            }
            // Not sure why everything else is assumed to be json (previously)
            else {
                line(unit() + "request = request.json(" + &param.name + ");") + NewLine
            }
        }
    };

    let multipart_parts: Result<Vec<RustPrinter>> = method
        .params
        .iter()
        .filter(|p| p.kind == ParamKind::Multipart)
        .map(make_part)
        .collect();

    let multipart_setter = if is_multipart {
        #[rustfmt::skip]
        let code = unit() +
            indent() + "let mut form = " + rust_name("reqwest::multipart", "Form") + "::new();" +
            (multipart_parts?.into_iter().map(|p| unit() + NewLine + p).reduce(|acc, e| acc + e). unwrap_or_else(unit) + NewLine) +
            NewLine +
            line("request = request.multipart(form);");

        code
    } else {
        unit()
    };

    let range_results = as_range(&method.result_status_code).is_some()
        || method.errors.codes.keys().any(|c| as_range(c).is_some());

    #[rustfmt::skip]
    let success_result_case = unit() +
        line(status_match(range_results, &method.result_status_code) +  " => {") +
        indented(
            line(unit() + "Ok(" + response_body_parsing(&method.result) + ")")
        ) +
        line("}");

    let error_cases = method
        .errors
        .codes
        .iter()
        .map(|(code, tpe)| error_case(range_results, code, tpe, &method.name, error_kind))
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    #[rustfmt::skip]
    let process_result = unit() +
        if range_results { line("match (status, status / 100) {") } else { line("match status {") } +
        indented(
            success_result_case +
            error_cases +
            line(unit() + "_ => Err(" + client_error() + "::unexpected(status, response.bytes().await?)),")
        ) +
        line("}");

    #[rustfmt::skip]
    let code = unit() +
        line(method.render_declaration(error_kind) + " {") +
        indented(
            url +
            NewLine +
            query_setters?.into_iter().map(|l| l + NewLine).reduce(|acc, e| acc + e). unwrap_or_else(unit) +
            line("let mut request = self") +
            indented(
                line(".context") +
                line(".client") +
                line(unit() + "." + &method.http_method + "(url.clone());")
            ) +
            NewLine +
            headers_and_logs +
            NewLine +
            line("if let Some(token) = self.context.bearer_token() {") +
            indented(
                line("request = request.bearer_auth(token);")
            ) +
            line("}") +
            NewLine +
            body_setter +
            multipart_setter +
            line("let response = request.send().await?;") +
            NewLine +
            line("let status = response.status().as_u16();") +
            process_result
        ) +
        line("}");

    Ok(code)
}

pub fn client_gen(
    open_api: &OpenAPI,
    tag: Option<Tag>,
    ref_cache: &mut RefCache,
    ignored_paths: &[&str],
) -> Result<Module> {
    let ignored_paths: HashSet<String> =
        HashSet::from_iter(ignored_paths.iter().map(|ip| ip.to_string()));

    let paths: HashMap<String, ReferenceOr<PathItem>> = open_api
        .paths
        .iter()
        .filter_map(|(path_key, path_item)| {
            if !ignored_paths.contains(path_key) && match_tag(&tag, path_item) {
                Some((path_key.clone(), path_item.clone()))
            } else {
                None
            }
        })
        .collect();

    let common_prefix = paths
        .keys()
        .map(|p| Path::from_string(p))
        .reduce(|acc, e| e.common_prefix(acc));

    let prefix_length = common_prefix.map(|p| p.0.len()).unwrap_or(0);

    let operations: Vec<PathOperation> = paths
        .iter()
        .flat_map(|(path, op)| tag_operations(&tag, path, op))
        .collect();

    let name = tag
        .map(|t| t.name)
        .unwrap_or("Other".to_string())
        .to_case(Case::UpperCamel);

    let methods: Vec<Method> = trait_methods(&operations, prefix_length, ref_cache)
        .map_err(|e| e.extend(format!("In Tag {}.", &name)))?;

    let common_error = methods.iter().map(|m| &m.errors).unique().count() == 1;

    let error_kind = if common_error {
        ErrorKind::Common {
            name: format!("{name}Error"),
        }
    } else {
        ErrorKind::Custom {
            prefix: name.clone(),
        }
    };

    let methods_rendered = methods
        .iter()
        .map(|m| line(m.render_declaration(&error_kind) + ";"))
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    let methods_impl: Result<Vec<RustPrinter>> = methods
        .iter()
        .map(|m| render_method_implementation(m, &error_kind))
        .collect();

    let methods_impl = methods_impl?
        .into_iter()
        .reduce(|acc, e| acc + NewLine + e)
        .unwrap_or_else(unit);

    let all_errors: Vec<(String, MethodErrors)> = if common_error {
        methods
            .first()
            .iter()
            .map(|m| (m.name.clone(), m.errors.clone()))
            .collect()
    } else {
        methods
            .iter()
            .map(|m| (m.name.clone(), m.errors.clone()))
            .collect()
    };

    let rendered_errors: Result<Vec<_>> = all_errors
        .iter()
        .map(|(method_name, errors)| render_errors(method_name, &error_kind, errors))
        .collect();

    let rendered_errors = rendered_errors?
        .into_iter()
        .map(|p| p + NewLine)
        .reduce(|acc, e| acc + e)
        .unwrap_or_else(unit);

    #[rustfmt::skip]
    let code = unit() +
        rendered_errors +
        line(async_annotation()) +
        line(unit() + "pub trait " + &name + "Client {") +
            indented(methods_rendered) +
        line("}") +
        NewLine +
        line(unit() + "pub struct " + &name + "ClientLive {") +
        indented(
            line(unit() + "pub context: " + rust_name("crate", "Context") + ",")
        ) +
        line("}") +
        NewLine +
        line(async_annotation()) +
        line(unit() + "impl " + &name + "Client for " + &name + "ClientLive {") +
        indented(
            methods_impl
        ) +
        line("}");

    let code = RustContext::new().print_to_string(code);

    let mut exports: Vec<String> = methods
        .iter()
        .map(|m| error_name(&m.name, &error_kind))
        .unique()
        .collect();

    exports.push(name.clone() + "Client");
    exports.push(name.clone() + "ClientLive");

    Ok(Module {
        def: ModuleDef {
            name: ModuleName::new(name.to_case(Case::Snake)),
            exports,
        },
        code,
    })
}
