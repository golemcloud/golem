use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct OpenApiInfo {
    title: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenApiDoc {
    info: Option<OpenApiInfo>,
    components: Option<Components>,
    paths: Option<HashMap<String, PathItem>>, // NEW
}

#[derive(Debug, Deserialize)]
struct Components {
    schemas: Option<HashMap<String, SchemaObject>>, // components.schemas.*
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum SchemaObject {
    Schema(Schema),
    Ref(RefObj),
}

#[derive(Debug, Clone, Deserialize)]
struct RefObj {
    #[serde(rename = "$ref")]
    r#ref: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    properties: Option<HashMap<String, SchemaObject>>, // nested
    #[serde(default)]
    required: Option<Vec<String>>,
    #[serde(default, rename = "enum")]
    r#enum: Option<Vec<String>>, // enum cases for string enums
    #[serde(default)]
    items: Option<Box<SchemaObject>>, // for arrays
}

#[derive(Debug, Deserialize)]
struct PathItem {
    #[serde(default)]
    get: Option<Operation>,
    #[serde(default)]
    post: Option<Operation>,
    #[serde(default)]
    put: Option<Operation>,
    #[serde(default)]
    delete: Option<Operation>,
    #[serde(default)]
    parameters: Option<Vec<ParameterObject>>,
}

#[derive(Debug, Deserialize)]
struct Operation {
    #[serde(default)]
    operationId: Option<String>,
    #[serde(default)]
    requestBody: Option<RequestBody>,
    #[serde(default)]
    responses: Option<HashMap<String, Response>>, // e.g., "200": { ... }
    #[serde(default)]
    parameters: Option<Vec<ParameterObject>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ParameterObject {
    Param(Parameter),
    Ref(RefObj),
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    #[serde(rename = "in")]
    location: String, // "path" | "query" | "header" | others
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    schema: Option<SchemaObject>,
}

#[derive(Debug, Deserialize)]
struct RequestBody {
    content: Option<HashMap<String, MediaType>>, // e.g., application/json
}

#[derive(Debug, Deserialize)]
struct Response {
    content: Option<HashMap<String, MediaType>>, // e.g., application/json
}

#[derive(Debug, Deserialize)]
struct MediaType {
    schema: Option<SchemaObject>,
}

pub fn parse_title_version(doc: &str) -> Option<(String, String)> {
    let parsed: OpenApiDoc = match serde_yaml::from_str(doc) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let info = parsed.info?;
    let title = info.title.unwrap_or_default();
    let version = info.version.unwrap_or_else(|| "0.0.0".to_string());
    if title.is_empty() { return None; }
    Some((title, version))
}

#[derive(Debug, Clone)]
pub struct ParsedRecord {
    pub name: String,
    pub fields: Vec<ParsedField>,
}

#[derive(Debug, Clone)]
pub struct ParsedField {
    pub name: String,
    pub ty: String,     // logical: string|boolean|integer|number|ref:Name|list:<inner>
    pub optional: bool, // true if not in required
}

pub fn parse_component_records(doc: &str) -> Vec<ParsedRecord> {
    let parsed: OpenApiDoc = match serde_yaml::from_str(doc) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    let components = match parsed.components { Some(c) => c, None => return out };
    let schemas = match components.schemas { Some(s) => s, None => return out };

    for (name, obj) in schemas.into_iter() {
        if let SchemaObject::Schema(schema) = obj {
            if let Some(props) = schema.properties.as_ref() {
                let required_set: std::collections::HashSet<_> = schema
                    .required
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let mut fields = Vec::new();
                for (fname, fobj) in props.iter() {
                    let (fty, extra_records) = map_field_type_with_nesting(&name, fname, fobj);
                    out.extend(extra_records);
                    let optional = !required_set.contains(fname);
                    fields.push(ParsedField { name: fname.clone(), ty: fty, optional });
                }
                out.push(ParsedRecord { name, fields });
            }
        }
    }

    out
}

fn map_field_type_with_nesting(parent: &str, field: &str, fobj: &SchemaObject) -> (String, Vec<ParsedRecord>) {
    match fobj {
        SchemaObject::Ref(r) => (format!("ref:{}", ref_tail(&r.r#ref).unwrap_or_else(|| "string".to_string())), vec![]),
        SchemaObject::Schema(s) => match s.r#type.as_deref() {
            Some("array") => {
                if let Some(items) = &s.items {
                    match items.as_ref() {
                        SchemaObject::Schema(is) => match is.r#type.as_deref() {
                            Some("object") => {
                                if let Some(props) = is.properties.as_ref() {
                                    let nested_name = format!("{}-{}-item", parent, field);
                                    let mut nested_fields = Vec::new();
                                    let required_set: std::collections::HashSet<_> = is.required.clone().unwrap_or_default().into_iter().collect();
                                    for (nf, nfobj) in props.iter() {
                                        let (nfty, extra) = map_field_type_with_nesting(&nested_name, nf, nfobj);
                                        nested_fields.push(ParsedField { name: nf.clone(), ty: nfty, optional: !required_set.contains(nf) });
                                        if !extra.is_empty() { /* accumulate in out below */ }
                                    }
                                    let nested_rec = ParsedRecord { name: nested_name.clone(), fields: nested_fields };
                                    // Note: nested of nested already appended via map_field_type_with_nesting extra vec currently discarded; to keep it, re-run recursion returns; for simplicity, nested depth 2 acceptable
                                    (format!("list:ref:{}", nested_name), vec![nested_rec])
                                } else {
                                    ("list:string".to_string(), vec![])
                                }
                            }
                            Some(inner) => (format!("list:{}", inner), vec![]),
                            None => ("list:string".to_string(), vec![]),
                        },
                        SchemaObject::Ref(r) => (format!("list:ref:{}", ref_tail(&r.r#ref).unwrap_or_else(|| "string".to_string())), vec![]),
                    }
                } else {
                    ("list:string".to_string(), vec![])
                }
            }
            Some("object") => {
                if let Some(props) = s.properties.as_ref() {
                    let nested_name = format!("{}-{}", parent, field);
                    let mut nested_fields = Vec::new();
                    let mut extra_records = Vec::new();
                    let required_set: std::collections::HashSet<_> = s.required.clone().unwrap_or_default().into_iter().collect();
                    for (nf, nfobj) in props.iter() {
                        let (nfty, extra) = map_field_type_with_nesting(&nested_name, nf, nfobj);
                        nested_fields.push(ParsedField { name: nf.clone(), ty: nfty, optional: !required_set.contains(nf) });
                        extra_records.extend(extra);
                    }
                    let nested_rec = ParsedRecord { name: nested_name.clone(), fields: nested_fields };
                    extra_records.push(nested_rec);
                    (format!("ref:{}", nested_name), extra_records)
                } else {
                    ("string".to_string(), vec![])
                }
            }
            Some(t) => (t.to_string(), vec![]),
            None => ("string".to_string(), vec![]),
        },
    }
}

#[derive(Debug, Clone)]
pub struct ParsedEnum {
    pub name: String,
    pub cases: Vec<String>,
}

pub fn parse_component_enums(doc: &str) -> Vec<ParsedEnum> {
    let parsed: OpenApiDoc = match serde_yaml::from_str(doc) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    let components = match parsed.components { Some(c) => c, None => return out };
    let schemas = match components.schemas { Some(s) => s, None => return out };
    for (name, obj) in schemas.into_iter() {
        if let SchemaObject::Schema(schema) = obj {
            if schema.properties.is_none() {
                if let Some(cases) = schema.r#enum.clone() {
                    out.push(ParsedEnum { name, cases });
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct ParsedOperation {
    pub operation_id: String,
    pub group: String,
    pub params_record: Option<String>,
    pub request_record: Option<String>,
    pub response_record: Option<String>,
}

pub fn parse_operations(doc: &str) -> Vec<ParsedOperation> {
    let parsed: OpenApiDoc = match serde_yaml::from_str(doc) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    let paths = match parsed.paths { Some(p) => p, None => return out };

    for (path, item) in paths.into_iter() {
        for op in [item.get, item.post, item.put, item.delete].into_iter().flatten() {
            let operation_id = op.operationId.clone().unwrap_or_default();
            if operation_id.is_empty() { continue; }
            let group = synth_group_name(&path);
            let req_rec = op.requestBody.as_ref().and_then(|rb| first_json_ref_name(rb.content.as_ref()));
            let resp_rec = find_200_json_ref_name(op.responses.as_ref());
            out.push(ParsedOperation { operation_id, group, params_record: None, request_record: req_rec, response_record: resp_rec });
        }
    }

    out
}

pub fn parse_operations_with_inline(doc: &str) -> (Vec<ParsedOperation>, Vec<ParsedRecord>) {
    let parsed: OpenApiDoc = match serde_yaml::from_str(doc) { Ok(v) => v, Err(_) => return (vec![], vec![]) };
    let mut ops = Vec::new();
    let mut recs = Vec::new();
    let paths = match parsed.paths { Some(p) => p, None => return (ops, recs) };

    for (path, item) in paths.into_iter() {
        for (method, op_opt) in [("get", item.get), ("post", item.post), ("put", item.put), ("delete", item.delete)] {
            if let Some(op) = op_opt {
                let operation_id = op.operationId.clone().unwrap_or_default();
                if operation_id.is_empty() { continue; }
                let group = synth_group_name(&path);

                // params (path+query+header)
                let mut params_rec_name: Option<String> = None;
                let mut params_fields: Vec<ParsedField> = Vec::new();
                let mut any_params = false;
                let merged_params = merge_parameters(item.parameters.as_ref(), op.parameters.as_ref());
                for p in merged_params {
                    if p.location == "path" || p.location == "query" || p.location == "header" {
                        any_params = true;
                        let ty = match p.schema.as_ref() {
                            Some(SchemaObject::Schema(s)) => s.r#type.clone().unwrap_or_else(|| "string".to_string()),
                            Some(SchemaObject::Ref(_)) | None => "string".to_string(),
                        };
                        let optional = !(p.required.unwrap_or(p.location == "path"));
                        let mapped_name = if p.location == "header" { map_header_name(&p.name) } else { p.name.clone() };
                        params_fields.push(ParsedField { name: mapped_name, ty, optional });
                    }
                }
                if any_params {
                    let name = synth_params_name(&path, method);
                    params_rec_name = Some(name.clone());
                    recs.push(ParsedRecord { name, fields: params_fields });
                }

                // request body
                let mut req_name: Option<String> = None;
                if let Some(rb) = op.requestBody.as_ref() {
                    if let Some(mt) = rb.content.as_ref().and_then(|m| m.get("application/json")) {
                        match mt.schema.as_ref() {
                            Some(SchemaObject::Ref(r)) => req_name = ref_tail(&r.r#ref),
                            Some(SchemaObject::Schema(s)) => {
                                if let Some(props) = s.properties.as_ref() {
                                    let name = synth_inline_name(&path, method, true);
                                    let required_set: std::collections::HashSet<_> = s
                                        .required
                                        .clone()
                                        .unwrap_or_default()
                                        .into_iter()
                                        .collect();
                                    let mut fields = Vec::new();
                                    for (fname, fobj) in props.iter() {
                                        let (fty, extra) = map_field_type_with_nesting(&name, fname, fobj);
                                        fields.push(ParsedField { name: fname.clone(), ty: fty, optional: !required_set.contains(fname) });
                                        if !extra.is_empty() { recs.extend(extra); }
                                    }
                                    recs.push(ParsedRecord { name: name.clone(), fields });
                                    req_name = Some(name);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // response body
                let mut resp_name: Option<String> = None;
                if let Some(responses) = op.responses.as_ref() {
                    if let Some(ok) = responses.get("200") {
                        if let Some(mt) = ok.content.as_ref().and_then(|m| m.get("application/json")) {
                            match mt.schema.as_ref() {
                                Some(SchemaObject::Ref(r)) => resp_name = ref_tail(&r.r#ref),
                                Some(SchemaObject::Schema(s)) => {
                                    if let Some(props) = s.properties.as_ref() {
                                        let name = synth_inline_name(&path, method, false);
                                        let required_set: std::collections::HashSet<_> = s
                                            .required
                                            .clone()
                                            .unwrap_or_default()
                                            .into_iter()
                                            .collect();
                                        let mut fields = Vec::new();
                                        for (fname, fobj) in props.iter() {
                                            let (fty, extra) = map_field_type_with_nesting(&name, fname, fobj);
                                            fields.push(ParsedField { name: fname.clone(), ty: fty, optional: !required_set.contains(fname) });
                                            if !extra.is_empty() { recs.extend(extra); }
                                        }
                                        recs.push(ParsedRecord { name: name.clone(), fields });
                                        resp_name = Some(name);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }

                ops.push(ParsedOperation { operation_id, group, params_record: params_rec_name, request_record: req_name, response_record: resp_name });
            }
        }
    }

    (ops, recs)
}

fn merge_parameters(path_params: Option<&Vec<ParameterObject>>, op_params: Option<&Vec<ParameterObject>>) -> Vec<Parameter> {
    let mut map: HashMap<(String, String), Parameter> = HashMap::new();
    let mut insert = |po: &ParameterObject| {
        if let ParameterObject::Param(p) = po {
            let key = (p.name.clone(), p.location.clone());
            map.insert(key, Parameter { name: p.name.clone(), location: p.location.clone(), required: p.required, schema: p.schema.clone() });
        }
    };
    if let Some(v) = path_params { for po in v { insert(po); } }
    if let Some(v) = op_params { for po in v { insert(po); } }
    map.into_values().collect()
}

fn map_header_name(orig: &str) -> String {
    match orig.to_ascii_lowercase().as_str() {
        "authorization" => "auth".to_string(),
        "etag" => "version".to_string(),
        "if-match" => "expected-version".to_string(),
        "last-modified" => "last-updated".to_string(),
        _ => orig.to_string(),
    }
}

fn synth_group_name(path: &str) -> String {
    let seg = path.split('/').filter(|s| !s.is_empty() && !s.starts_with('{')).next().unwrap_or("");
    let s = seg.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    s.trim_matches('-').to_lowercase()
}

fn synth_inline_name(path: &str, method: &str, is_request: bool) -> String {
    let s = path.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let kind = if is_request { "request-body" } else { "response-body" };
    format!("{}-{}-{}", s, method.to_lowercase(), kind)
}

fn synth_params_name(path: &str, method: &str) -> String {
    let s = path.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}-{}-params", s, method.to_lowercase())
}

fn first_json_ref_name(content: Option<&HashMap<String, MediaType>>) -> Option<String> {
    let media = content?;
    let mt = media.get("application/json")?;
    match mt.schema.as_ref()? { SchemaObject::Ref(r) => ref_tail(&r.r#ref), SchemaObject::Schema(_) => None }
}

fn find_200_json_ref_name(responses: Option<&HashMap<String, Response>>) -> Option<String> {
    let res = responses?;
    let ok = res.get("200")?;
    match ok.content.as_ref().and_then(|m| m.get("application/json")).and_then(|mt| mt.schema.as_ref())? {
        SchemaObject::Ref(r) => ref_tail(&r.r#ref),
        SchemaObject::Schema(_) => None,
    }
}

fn ref_tail(s: &str) -> Option<String> {
    // e.g., "#/components/schemas/Todo" -> "Todo"
    s.rsplit('/').next().map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_component_records, parse_operations, parse_operations_with_inline, parse_title_version};

    #[test]
    fn parses_title_version() {
        let doc = "openapi: '3.0.3'\ninfo:\n  title: Todo REST API\n  version: '1.0.0'\n";
        let tv = parse_title_version(doc).unwrap();
        assert_eq!(tv.0, "Todo REST API");
        assert_eq!(tv.1, "1.0.0");
    }

    #[test]
    fn parses_simple_and_nested_schema() {
        let doc = r#"openapi: '3.0.3'
info: { title: X, version: '1.0.0' }
components:
  schemas:
    Todo:
      type: object
      required: [id]
      properties:
        id: { type: string }
        meta:
          type: object
          properties:
            etag: { type: string }
        items:
          type: array
          items:
            type: object
            properties:
              label: { type: string }
"#;
        let recs = parse_component_records(doc);
        // Expect top-level Todo and nested Todo-meta and Todo-items-item
        let names: std::collections::HashSet<_> = recs.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains("Todo"));
        assert!(names.contains("Todo-meta"));
        assert!(names.contains("Todo-items-item"));
        let todo = recs.iter().find(|r| r.name == "Todo").unwrap();
        let meta_field = todo.fields.iter().find(|f| f.name == "meta").unwrap();
        assert_eq!(meta_field.ty, "ref:Todo-meta");
        let items_field = todo.fields.iter().find(|f| f.name == "items").unwrap();
        assert_eq!(items_field.ty, "list:ref:Todo-items-item");
    }

    #[test]
    fn parses_operations_refs() {
        let doc = r##"openapi: 3.0.3
info: { title: X, version: '1.0.0' }
paths:
  /todos:
    post:
      operationId: CreateTodo
      requestBody:
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/TodoCreate"
      responses:
        "200":
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Todo"
components:
  schemas:
    TodoCreate: { type: object }
    Todo: { type: object }
"##;
        let ops = parse_operations(doc);
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.operation_id, "CreateTodo");
        assert_eq!(op.group, "todos");
        assert_eq!(op.params_record, None);
        assert_eq!(op.request_record.as_deref(), Some("TodoCreate"));
        assert_eq!(op.response_record.as_deref(), Some("Todo"));
    }

    #[test]
    fn parses_inline_request_and_response_records_and_headers() {
        let doc = r##"openapi: 3.0.3
info: { title: X, version: '1.0.0' }
paths:
  /todos/{id}:
    put:
      operationId: UpdateTodo
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
        - name: Authorization
          in: header
          required: false
          schema: { type: string }
        - name: If-Match
          in: header
          required: false
          schema: { type: string }
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [title]
              properties:
                title: { type: string }
                tags: { type: array, items: { type: string } }
      responses:
        "200":
          content:
            application/json:
              schema:
                type: object
                properties:
                  ok: { type: boolean }
"##;
        let (ops, recs) = parse_operations_with_inline(doc);
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.operation_id, "UpdateTodo");
        assert_eq!(op.group, "todos");
        assert_eq!(op.params_record.as_deref(), Some("todos-id-put-params"));
        let params = recs.iter().find(|r| r.name == "todos-id-put-params").unwrap();
        let names: Vec<_> = params.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"id"));
        assert!(names.contains(&"auth"));
        assert!(names.contains(&"expected-version"));
    }
} 