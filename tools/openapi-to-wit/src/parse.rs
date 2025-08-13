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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SchemaObject {
    Schema(Schema),
    Ref(RefObj),
}

#[derive(Debug, Deserialize)]
struct RefObj {
    #[serde(rename = "$ref")]
    r#ref: String,
}

#[derive(Debug, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
struct Operation {
    #[serde(default)]
    operationId: Option<String>,
    #[serde(default)]
    requestBody: Option<RequestBody>,
    #[serde(default)]
    responses: Option<HashMap<String, Response>>, // e.g., "200": { ... }
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
                    let (fty) = match fobj {
                        SchemaObject::Schema(s) => match s.r#type.as_deref() {
                            Some("array") => {
                                // Determine inner type
                                if let Some(items) = &s.items {
                                    match items.as_ref() {
                                        SchemaObject::Schema(is) => {
                                            let inner = is.r#type.clone().unwrap_or_else(|| "string".to_string());
                                            format!("list:{}", inner)
                                        }
                                        SchemaObject::Ref(r) => match ref_tail(&r.r#ref) {
                                            Some(name) => format!("list:ref:{}", name),
                                            None => "list:string".to_string(),
                                        }
                                    }
                                } else {
                                    "list:string".to_string()
                                }
                            }
                            Some(t) => t.to_string(),
                            None => "string".to_string(),
                        },
                        SchemaObject::Ref(r) => match ref_tail(&r.r#ref) {
                            Some(name) => format!("ref:{}", name),
                            None => "string".to_string(),
                        },
                    };
                    let optional = !required_set.contains(fname);
                    fields.push(ParsedField { name: fname.clone(), ty: fty, optional });
                }
                out.push(ParsedRecord { name, fields });
            }
        }
    }

    out
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

    for (_path, item) in paths.into_iter() {
        for op in [item.get, item.post, item.put, item.delete].into_iter().flatten() {
            let operation_id = op.operationId.clone().unwrap_or_default();
            if operation_id.is_empty() { continue; }

            let req_rec = op.requestBody.as_ref().and_then(|rb| first_json_ref_name(rb.content.as_ref()));
            let resp_rec = find_200_json_ref_name(op.responses.as_ref());

            out.push(ParsedOperation { operation_id, request_record: req_rec, response_record: resp_rec });
        }
    }

    out
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
    use super::{parse_component_enums, parse_component_records, parse_operations, parse_title_version};

    #[test]
    fn parses_title_version() {
        let doc = "openapi: '3.0.3'\ninfo:\n  title: Todo REST API\n  version: '1.0.0'\n";
        let tv = parse_title_version(doc).unwrap();
        assert_eq!(tv.0, "Todo REST API");
        assert_eq!(tv.1, "1.0.0");
    }

    #[test]
    fn parses_simple_schema() {
        let doc = r#"openapi: '3.0.3'
info: { title: X, version: '1.0.0' }
components:
  schemas:
    Todo:
      type: object
      required: [id, title]
      properties:
        id: { type: string }
        title: { type: string }
        completed: { type: boolean }
"#;
        let recs = parse_component_records(doc);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.name, "Todo");
        assert_eq!(r.fields.len(), 3);
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
        assert_eq!(op.request_record.as_deref(), Some("TodoCreate"));
        assert_eq!(op.response_record.as_deref(), Some("Todo"));
    }

    #[test]
    fn parses_component_enums() {
        let doc = r#"openapi: 3.0.3
info: { title: X, version: '1.0.0' }
components:
  schemas:
    Status:
      type: string
      enum: [open, closed]
"#;
        let enums = parse_component_enums(doc);
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Status");
        assert_eq!(enums[0].cases, vec!["open", "closed"]);
    }
} 