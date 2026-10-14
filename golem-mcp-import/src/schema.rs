//! JSON Schema projection and reversible JSON value conversion.

use base64::Engine;
use golem_schema::schema::{
    BinaryRestrictions, BinaryValuePayload, DiscriminatorRule, FieldDiscriminator,
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue, TypeId,
    UnionBranch, UnionSpec, UnionValuePayload,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

pub const EXTRAS: &str = "additional-properties";

// Validation can traverse a bounded reference chain at each bounded JSON level.
const CONVERSION_STACK: usize = 64 * 1024 * 1024;

struct NoExternalSchemas;

impl jsonschema::Retrieve for NoExternalSchemas {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is disabled".into())
    }
}

fn validator(source: &Value) -> Result<jsonschema::Validator, Error> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_retriever(NoExternalSchemas)
        .with_pattern_options(
            jsonschema::PatternOptions::regex()
                .size_limit(1 << 20)
                .dfa_size_limit(1 << 20),
        )
        .build(source)
        .map_err(|e| schema("/", &e.to_string()))
}

/// Resource limits applied before jsonschema compilation and on every value.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub schema_bytes: usize,
    pub schema_nodes: usize,
    pub schema_depth: usize,
    pub instance_bytes: usize,
    pub instance_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            schema_bytes: 1 << 20,
            schema_nodes: 16_384,
            schema_depth: 128,
            instance_bytes: 4 << 20,
            instance_depth: 256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootField {
    pub json_name: String,
    pub schema_name: String,
    pub required: bool,
}

/// Serializable admission-time projection, with a lazily rehydrated validator cache.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Projection {
    source: Value,
    graph: SchemaGraph,
    mapping: Node,
    definitions: BTreeMap<String, Node>,
    root_fields: Vec<RootField>,
    limits: Limits,
    #[serde(skip)]
    compiled: Arc<OnceLock<jsonschema::Validator>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Node {
    Annotated {
        node: Box<Node>,
        metadata: MetadataEnvelope,
    },
    Null,
    Bool,
    String,
    Json,
    Binary,
    Integer(IntKind),
    Number,
    Enum(Vec<String>),
    Array(Box<Node>),
    Tuple(Vec<Node>),
    Union {
        field: String,
        cases: Vec<(String, Node)>,
    },
    Object {
        fields: Vec<Field>,
        extras: Option<Box<Node>>,
    },
    Nullable(Box<Node>),
    Ref(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum IntKind {
    S8,
    S16,
    S32,
    S64,
    U8,
    U16,
    U32,
    U64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Field {
    json_name: String,
    schema_name: String,
    required: bool,
    node: Node,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("schema at {path}: {reason}")]
    Schema { path: String, reason: String },
    #[error("value at {path}: {reason}")]
    Value { path: String, reason: String },
    #[error("resource limit exceeded: {0}")]
    Limit(String),
}

impl Projection {
    pub fn new(source: Value, limits: Limits) -> Result<Self, Error> {
        stacker::maybe_grow(CONVERSION_STACK, CONVERSION_STACK, || {
            Self::build(source, limits)
        })
    }

    fn build(source: Value, limits: Limits) -> Result<Self, Error> {
        if limits.schema_depth > Limits::default().schema_depth
            || limits.instance_depth > Limits::default().instance_depth
        {
            return Err(Error::Limit(
                "requested depth exceeds supported recursion bounds".into(),
            ));
        }
        check_tree(
            &source,
            limits.schema_bytes,
            limits.schema_nodes,
            limits.schema_depth,
            "schema",
        )?;
        reject_ref_machinery(&source, "")?;
        check_validation_depth(
            &source,
            &source,
            limits.schema_depth,
            &mut limits.schema_nodes.clone(),
            &mut BTreeSet::new(),
        )?;
        jsonschema::draft202012::meta::validate(&source)
            .map_err(|e| schema("/", &e.to_string()))?;
        let compiled = Arc::new(OnceLock::from(validator(&source)?));

        let mut p = Projector {
            source: &source,
            defs: BTreeMap::new(),
            namespace: blake3::hash(source.to_string().as_bytes())
                .to_hex()
                .to_string(),
            limits,
            projecting: BTreeSet::new(),
            remaining: limits.schema_nodes,
        };
        let mapping = p.project(&source, "#", 0)?;
        let definitions = p.defs;
        let graph = SchemaGraph {
            defs: definitions
                .iter()
                .map(|(id, node)| {
                    Ok(SchemaTypeDef {
                        id: TypeId::new(id.clone()),
                        name: None,
                        body: node.ty(&definitions)?,
                    })
                })
                .collect::<Result<_, Error>>()?,
            root: mapping.ty(&definitions)?,
        };
        golem_schema::schema::validation::validate_graph(&graph)
            .map_err(|e| schema("/", &format!("projected graph is not well formed: {e:?}")))?;
        let root_fields = match mapping.resolve(&definitions) {
            Node::Object { fields, .. } => fields
                .iter()
                .map(|f| RootField {
                    json_name: f.json_name.clone(),
                    schema_name: f.schema_name.clone(),
                    required: f.required,
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            source,
            graph,
            mapping,
            definitions,
            root_fields,
            limits,
            compiled,
        })
    }

    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }
    pub(crate) fn source(&self) -> &Value {
        &self.source
    }
    pub fn root(&self) -> &SchemaType {
        &self.graph.root
    }
    pub fn root_fields(&self) -> &[RootField] {
        &self.root_fields
    }

    pub fn is_object(&self) -> bool {
        matches!(
            self.mapping.resolve(&self.definitions),
            Node::Object { .. } | Node::Union { .. }
        )
    }

    pub fn from_json(&self, value: &Value) -> Result<SchemaValue, Error> {
        stacker::maybe_grow(CONVERSION_STACK, CONVERSION_STACK, || {
            check_tree(
                value,
                self.limits.instance_bytes,
                usize::MAX,
                self.limits.instance_depth,
                "instance",
            )?;
            self.validate(value)?;
            encode(&self.mapping, value, &self.definitions, "")
        })
    }

    pub fn to_json(&self, value: &SchemaValue) -> Result<Value, Error> {
        stacker::maybe_grow(CONVERSION_STACK, CONVERSION_STACK, || {
            check_value(value, self.limits)?;
            let json = decode(&self.mapping, value, &self.definitions, "")?;
            check_tree(
                &json,
                self.limits.instance_bytes,
                usize::MAX,
                self.limits.instance_depth,
                "instance",
            )?;
            self.validate(&json)?;
            Ok(json)
        })
    }

    fn validate(&self, value: &Value) -> Result<(), Error> {
        if self.compiled.get().is_none() {
            let _ = self.compiled.set(validator(&self.source)?);
        }
        let validator = self.compiled.get().unwrap();
        if let Some(e) = validator.iter_errors(value).next() {
            return Err(Error::Value {
                path: e.instance_path().to_string(),
                reason: e.to_string(),
            });
        }
        Ok(())
    }
}

struct Projector<'a> {
    source: &'a Value,
    defs: BTreeMap<String, Node>,
    namespace: String,
    limits: Limits,
    projecting: BTreeSet<String>,
    remaining: usize,
}

impl Projector<'_> {
    fn project(&mut self, s: &Value, path: &str, depth: usize) -> Result<Node, Error> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| Error::Limit("expanded schema nodes".into()))?;
        let node = self.project_shape(s, path, depth)?;
        let mut docs = Vec::new();
        for key in ["title", "description"] {
            if let Some(value) = s.get(key).and_then(Value::as_str) {
                docs.push(value.to_owned());
            }
        }
        for key in ["default", "examples", "format"] {
            if let Some(value) = s.get(key) {
                docs.push(format!("{key}: {value}"));
            }
        }
        let metadata = MetadataEnvelope {
            doc: (!docs.is_empty()).then(|| docs.join("\n\n")),
            deprecated: (s.get("deprecated") == Some(&Value::Bool(true)))
                .then(|| "Deprecated upstream".into()),
            ..Default::default()
        };
        Ok(if metadata.is_empty() {
            node
        } else {
            Node::Annotated {
                node: Box::new(node),
                metadata,
            }
        })
    }

    fn project_shape(&mut self, s: &Value, path: &str, depth: usize) -> Result<Node, Error> {
        if depth > self.limits.schema_depth {
            return Err(Error::Limit("schema projection depth".into()));
        }
        if let Some(r) = s.get("$ref").and_then(Value::as_str) {
            if s.as_object().is_some_and(|o| {
                o.keys().any(|k| {
                    !matches!(
                        k.as_str(),
                        "$ref"
                            | "$defs"
                            | "$schema"
                            | "$id"
                            | "title"
                            | "description"
                            | "default"
                            | "examples"
                            | "format"
                            | "$comment"
                            | "deprecated"
                            | "readOnly"
                            | "writeOnly"
                    )
                })
            }) {
                return Err(schema(
                    path,
                    "$ref siblings cannot be projected without weakening them",
                ));
            }
            if !r.starts_with('#') {
                return Err(schema(path, "only local $ref is supported"));
            }
            let id = format!("mcp.schema.{}.{}", self.namespace, ref_id(r));
            if !self.defs.contains_key(&id) && self.projecting.insert(id.clone()) {
                let target =
                    pointer(self.source, r).ok_or_else(|| schema(path, "unresolved local $ref"))?;
                let n = self.project(target, r, depth + 1)?;
                self.defs.insert(id.clone(), n);
                self.projecting.remove(&id);
            }
            return Ok(Node::Ref(id));
        }
        if s == &Value::Bool(true) || s.as_object().is_some_and(|x| x.is_empty()) {
            return Ok(Node::Json);
        }
        if s == &Value::Bool(false) {
            return Err(schema(
                path,
                "the unsatisfiable false schema has no Golem representation",
            ));
        }
        if let Some(branches) = s.get("allOf").and_then(Value::as_array) {
            let mut shape = s.as_object().unwrap().clone();
            shape.remove("allOf");
            let mut pending: Vec<_> = branches.iter().map(|b| (b, depth + 1)).collect();
            while let Some((branch, depth)) = pending.pop() {
                self.remaining = self
                    .remaining
                    .checked_sub(1)
                    .ok_or_else(|| Error::Limit("expanded allOf nodes".into()))?;
                if depth > self.limits.schema_depth {
                    return Err(Error::Limit("allOf projection depth".into()));
                }
                let Some(branch) = branch.as_object() else {
                    if branch == &Value::Bool(false) {
                        return Err(schema(path, "unsatisfiable allOf branch"));
                    }
                    continue;
                };
                for (key, value) in branch {
                    match key.as_str() {
                        "allOf" => {
                            pending.extend(value.as_array().unwrap().iter().map(|b| (b, depth + 1)))
                        }
                        "properties" => {
                            let props = shape
                                .entry(key.clone())
                                .or_insert_with(|| Value::Object(Map::new()))
                                .as_object_mut()
                                .unwrap();
                            for (name, property) in value.as_object().unwrap() {
                                match props.get_mut(name) {
                                    Some(existing) => {
                                        *existing =
                                            serde_json::json!({"allOf":[existing,property]});
                                        check_tree(
                                            existing,
                                            self.limits.schema_bytes,
                                            self.limits.schema_nodes,
                                            self.limits.schema_depth,
                                            "combined property",
                                        )?;
                                    }
                                    None => {
                                        props.insert(name.clone(), property.clone());
                                    }
                                }
                            }
                        }
                        "required" => {
                            let required = shape
                                .entry(key.clone())
                                .or_insert_with(|| Value::Array(Vec::new()))
                                .as_array_mut()
                                .unwrap();
                            for name in value.as_array().unwrap() {
                                if !required.contains(name) {
                                    required.push(name.clone());
                                }
                            }
                        }
                        "$ref" => {
                            let reference = value.as_str().unwrap();
                            let target = pointer(self.source, reference)
                                .ok_or_else(|| schema(path, "unresolved local allOf reference"))?;
                            pending.push((target, depth + 1));
                        }
                        _ => {
                            shape.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                    }
                }
            }
            let shape = Value::Object(shape);
            check_tree(
                &shape,
                self.limits.schema_bytes,
                self.limits.schema_nodes,
                self.limits.schema_depth,
                "combined schema",
            )?;
            return self.project(&shape, path, depth + 1);
        }
        if let Some(a) = s
            .get("oneOf")
            .or_else(|| s.get("anyOf"))
            .and_then(Value::as_array)
        {
            if s.get("type").is_some()
                && a.iter().all(|branch| {
                    branch.as_object().is_some_and(|object| {
                        object.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "required"
                                    | "minLength"
                                    | "maxLength"
                                    | "pattern"
                                    | "minimum"
                                    | "maximum"
                                    | "exclusiveMinimum"
                                    | "exclusiveMaximum"
                                    | "multipleOf"
                                    | "minProperties"
                                    | "maxProperties"
                                    | "dependentRequired"
                                    | "not"
                                    | "if"
                                    | "then"
                                    | "else"
                            )
                        })
                    })
                })
            {
                let mut shape = s.as_object().unwrap().clone();
                shape.remove("oneOf");
                shape.remove("anyOf");
                return self.project(&Value::Object(shape), path, depth + 1);
            }
            let mut nullable = false;
            let mut nodes = Vec::new();
            for (i, branch) in a.iter().enumerate() {
                let node = self.project(branch, &format!("{path}/branch/{i}"), depth + 1)?;
                match node.resolve(&self.defs) {
                    Node::Null => nullable = true,
                    Node::Nullable(inner) => {
                        nullable = true;
                        nodes.push(*inner.clone());
                    }
                    _ => nodes.push(node),
                }
            }
            let node = match nodes.len() {
                0 => return Ok(Node::Null),
                1 => nodes.pop().unwrap(),
                _ => self.union(nodes, path)?,
            };
            return Ok(if nullable && !node.nullable(&self.defs) {
                Node::Nullable(Box::new(node))
            } else {
                node
            });
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array) {
            if let Some(cases) = e
                .iter()
                .map(|x| x.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
            {
                return Ok(Node::Enum(cases));
            }
            if s.get("type").is_none() && e.iter().all(Value::is_number) {
                return Ok(
                    if e.iter()
                        .all(|n| n.as_f64().is_some_and(|f| f.fract() == 0.0))
                    {
                        Node::Integer(
                            if e.iter()
                                .any(|n| n.as_u64().is_some_and(|u| u > i64::MAX as u64))
                            {
                                if e.iter().any(|n| n.as_f64().is_some_and(|f| f < 0.0)) {
                                    return Err(schema(
                                        path,
                                        "numeric enum exceeds one integer domain",
                                    ));
                                }
                                IntKind::U64
                            } else {
                                IntKind::S64
                            },
                        )
                    } else {
                        Node::Number
                    },
                );
            }
        }
        if (s.get("type").is_none() || s.get("type").and_then(Value::as_str) == Some("string"))
            && let Some(value) = s.get("const")
        {
            return Ok(match value {
                Value::String(s) => Node::Enum(vec![s.clone()]),
                Value::Null => Node::Null,
                Value::Bool(_) => Node::Bool,
                Value::Number(_) if value.as_i64().is_some() => Node::Integer(IntKind::S64),
                Value::Number(_) if value.as_u64().is_some() => Node::Integer(IntKind::U64),
                Value::Number(_) => Node::Number,
                _ => {
                    return Err(schema(
                        path,
                        "composite const needs an explicit structural type",
                    ));
                }
            });
        }
        let types = s.get("type").and_then(Value::as_array);
        let nullable = types.is_some_and(|a| a.iter().any(|x| x == "null"));
        if types.is_some_and(|a| a.iter().filter(|x| *x != "null").count() != 1) {
            return Err(schema(
                path,
                "scalar type unions cannot be projected faithfully",
            ));
        }
        let typ = s
            .get("type")
            .and_then(|x| {
                x.as_str().or_else(|| {
                    x.as_array()?
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|x| *x != "null")
                })
            })
            .ok_or_else(|| schema(path, "schema must have a projectable type"))?;
        let n = match typ {
            "boolean" => Node::Bool,
            "string" if s.get("contentEncoding").and_then(Value::as_str) == Some("base64") => {
                Node::Binary
            }
            "string" => Node::String,
            "number" => Node::Number,
            "integer" => Node::Integer(integer_kind(s, path)?),
            "array" => {
                if let Some(prefix) = s.get("prefixItems").and_then(Value::as_array) {
                    let length = prefix.len() as u64;
                    if s.get("minItems").and_then(Value::as_u64) != Some(length)
                        || !(s.get("items") == Some(&Value::Bool(false))
                            || s.get("maxItems").and_then(Value::as_u64) == Some(length))
                    {
                        return Err(schema(path, "tuple arrays require a fixed length"));
                    }
                    Node::Tuple(
                        prefix
                            .iter()
                            .enumerate()
                            .map(|(i, x)| {
                                self.project(x, &format!("{path}/prefixItems/{i}"), depth + 1)
                            })
                            .collect::<Result<_, _>>()?,
                    )
                } else {
                    Node::Array(Box::new(self.project(
                        s.get("items").unwrap_or(&Value::Bool(true)),
                        &format!("{path}/items"),
                        depth + 1,
                    )?))
                }
            }
            "object" => self.object(s, path, depth)?,
            "null" => Node::Null,
            _ => return Err(schema(path, "unsupported type")),
        };
        Ok(if nullable {
            Node::Nullable(Box::new(n))
        } else {
            n
        })
    }

    fn union(&mut self, mut nodes: Vec<Node>, path: &str) -> Result<Node, Error> {
        if nodes
            .iter()
            .all(|node| matches!(node.resolve(&self.defs), Node::Enum(_)))
        {
            let mut values = BTreeSet::new();
            for node in &nodes {
                let Node::Enum(cases) = node.resolve(&self.defs) else {
                    unreachable!()
                };
                values.extend(cases.iter().cloned());
            }
            return Ok(Node::Enum(values.into_iter().collect()));
        }
        let mut tags = Vec::new();
        for node in &nodes {
            let Node::Object { fields, .. } = node.resolve(&self.defs) else {
                return Err(schema(
                    path,
                    "union needs record branches with disjoint required string tags",
                ));
            };
            tags.push(
                fields
                    .iter()
                    .filter_map(|field| {
                        if !field.required {
                            return None;
                        }
                        let Node::Enum(cases) = field.node.resolve(&self.defs) else {
                            return None;
                        };
                        (cases.len() == 1).then(|| (field.json_name.clone(), cases[0].clone()))
                    })
                    .collect::<BTreeMap<_, _>>(),
            );
        }
        let field = tags
            .first()
            .into_iter()
            .flat_map(|t| t.keys())
            .find(|key| {
                let values = tags
                    .iter()
                    .filter_map(|t| t.get(*key))
                    .collect::<BTreeSet<_>>();
                values.len() == nodes.len()
            })
            .cloned()
            .ok_or_else(|| schema(path, "union has no disjoint required string tag"))?;
        let mut cases = Vec::new();
        for (i, node) in nodes.drain(..).enumerate() {
            let mut body = node.resolve(&self.defs).clone();
            let Node::Object { fields, .. } = &mut body else {
                unreachable!()
            };
            let tag = fields.iter_mut().find(|f| f.json_name == field).unwrap();
            tag.node = Node::String;
            cases.push((tags[i][&field].clone(), body));
        }
        Ok(Node::Union { field, cases })
    }

    fn object(&mut self, s: &Value, path: &str, depth: usize) -> Result<Node, Error> {
        let props = s
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if props.contains_key(EXTRAS) {
            return Err(schema(
                path,
                "property conflicts with reserved synthetic additional-properties field",
            ));
        }
        let required: BTreeSet<&str> = s
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let mut fields = Vec::new();
        for (name, child) in props {
            let node = self.project(&child, &format!("{path}/properties/{name}"), depth + 1)?;
            let is_required = required.contains(name.as_str());
            fields.push(Field {
                json_name: name.clone(),
                schema_name: name,
                required: is_required,
                node,
            });
        }
        let extras = if s
            .get("patternProperties")
            .and_then(Value::as_object)
            .is_some_and(|p| !p.is_empty())
        {
            // Pattern matches can overlap; the original validator checks every applicable schema.
            Some(Box::new(Node::Json))
        } else {
            match s.get("additionalProperties") {
                Some(Value::Bool(false)) => None,
                Some(v) => Some(Box::new(self.project(
                    v,
                    &format!("{path}/additionalProperties"),
                    depth + 1,
                )?)),
                None => Some(Box::new(Node::Json)),
            }
        };
        Ok(Node::Object { fields, extras })
    }
}

impl Node {
    fn resolve<'a>(&'a self, defs: &'a BTreeMap<String, Node>) -> &'a Node {
        let mut node = self;
        let mut seen = BTreeSet::new();
        loop {
            match node {
                Node::Annotated { node: inner, .. } => node = inner,
                Node::Ref(id) => {
                    if !seen.insert(id) {
                        break;
                    }
                    let Some(target) = defs.get(id) else {
                        break;
                    };
                    node = target;
                }
                _ => break,
            }
        }
        node
    }

    fn nullable(&self, defs: &BTreeMap<String, Node>) -> bool {
        matches!(self.resolve(defs), Node::Nullable(_))
    }

    fn ty(&self, defs: &BTreeMap<String, Node>) -> Result<SchemaType, Error> {
        let m = MetadataEnvelope::default();
        Ok(match self {
            Node::Annotated { node, metadata } => {
                let mut ty = node.ty(defs)?;
                *ty.metadata_mut() = metadata.clone();
                ty
            }
            Node::Null => SchemaType::tuple(Vec::new()),
            Node::Bool => SchemaType::bool(),
            Node::String | Node::Json => SchemaType::string(),
            Node::Binary => SchemaType::binary(BinaryRestrictions::default()),
            Node::Number => SchemaType::f64(),
            Node::Integer(k) => match k {
                IntKind::S8 => SchemaType::s8(),
                IntKind::S16 => SchemaType::s16(),
                IntKind::S32 => SchemaType::s32(),
                IntKind::S64 => SchemaType::s64(),
                IntKind::U8 => SchemaType::u8(),
                IntKind::U16 => SchemaType::u16(),
                IntKind::U32 => SchemaType::u32(),
                IntKind::U64 => SchemaType::u64(),
            },
            Node::Enum(cases) => SchemaType::Enum {
                cases: cases.clone(),
                metadata: m,
            },
            Node::Array(x) => SchemaType::list(x.ty(defs)?),
            Node::Tuple(elements) => SchemaType::tuple(
                elements
                    .iter()
                    .map(|x| x.ty(defs))
                    .collect::<Result<_, _>>()?,
            ),
            Node::Union { field, cases } => SchemaType::Union {
                spec: UnionSpec {
                    branches: cases
                        .iter()
                        .enumerate()
                        .map(|(i, (tag, node))| {
                            Ok(UnionBranch {
                                tag: format!("case-{i}"),
                                body: node.ty(defs)?,
                                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                                    field_name: field.clone(),
                                    literal: Some(tag.clone()),
                                }),
                                metadata: MetadataEnvelope::default(),
                            })
                        })
                        .collect::<Result<_, Error>>()?,
                },
                metadata: m,
            },
            Node::Nullable(x) => SchemaType::option(x.ty(defs)?),
            Node::Ref(r) => {
                let mut target = r;
                let mut seen = BTreeSet::new();
                let mut metadata = MetadataEnvelope::default();
                while seen.insert(target) {
                    let mut node = defs.get(target);
                    while let Some(Node::Annotated {
                        node: inner,
                        metadata: annotation,
                    }) = node
                    {
                        if metadata.is_empty() {
                            metadata = annotation.clone();
                        }
                        node = Some(inner);
                    }
                    match node {
                        Some(Node::Ref(next)) => target = next,
                        _ => break,
                    }
                }
                let mut reference = SchemaType::ref_to(TypeId::new(target.clone()));
                *reference.metadata_mut() = metadata;
                reference
            }
            Node::Object { fields, extras } => {
                let mut fs = Vec::new();
                for f in fields {
                    let t = f.node.ty(defs)?;
                    let metadata = t.metadata().clone();
                    let body = if f.required {
                        t
                    } else if f.node.nullable(defs) {
                        SchemaType::option(SchemaType::Record {
                            fields: vec![NamedFieldType {
                                name: "value".into(),
                                body: t,
                                metadata: m.clone(),
                            }],
                            metadata: m.clone(),
                        })
                    } else {
                        SchemaType::option(t)
                    };
                    fs.push(NamedFieldType {
                        name: f.schema_name.clone(),
                        body,
                        metadata,
                    });
                }
                if let Some(x) = extras {
                    fs.push(NamedFieldType {
                        name: EXTRAS.into(),
                        body: SchemaType::Map {
                            key: Box::new(SchemaType::string()),
                            value: Box::new(x.ty(defs)?),
                            metadata: m.clone(),
                        },
                        metadata: m.clone(),
                    });
                }
                SchemaType::Record {
                    fields: fs,
                    metadata: m,
                }
            }
        })
    }
}

fn encode(
    n: &Node,
    v: &Value,
    defs: &BTreeMap<String, Node>,
    path: &str,
) -> Result<SchemaValue, Error> {
    let bad = |r: &str| Error::Value {
        path: path.into(),
        reason: r.into(),
    };
    Ok(match n.resolve(defs) {
        Node::Annotated { .. } | Node::Ref(_) => return Err(bad("unresolved mapping reference")),
        Node::Null if v.is_null() => SchemaValue::Tuple {
            elements: Vec::new(),
        },
        Node::Null => return Err(bad("expected null")),
        Node::Tuple(nodes) => {
            let values = v
                .as_array()
                .filter(|v| v.len() == nodes.len())
                .ok_or_else(|| bad("tuple length mismatch"))?;
            SchemaValue::Tuple {
                elements: nodes
                    .iter()
                    .zip(values)
                    .enumerate()
                    .map(|(i, (n, v))| encode(n, v, defs, &format!("{path}/{i}")))
                    .collect::<Result<_, _>>()?,
            }
        }
        Node::Union { field, cases } => {
            let tag = v
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| bad("missing string union tag"))?;
            let (i, (_, node)) = cases
                .iter()
                .enumerate()
                .find(|(_, (t, _))| t == tag)
                .ok_or_else(|| bad("unknown union tag"))?;
            SchemaValue::Union(UnionValuePayload {
                tag: format!("case-{i}"),
                body: Box::new(encode(node, v, defs, path)?),
            })
        }
        Node::Json => {
            SchemaValue::String(serde_json::to_string(v).map_err(|e| bad(&e.to_string()))?)
        }
        Node::Bool => SchemaValue::Bool(v.as_bool().ok_or_else(|| bad("expected boolean"))?),
        Node::String => {
            SchemaValue::String(v.as_str().ok_or_else(|| bad("expected string"))?.into())
        }
        Node::Binary => SchemaValue::Binary(BinaryValuePayload {
            bytes: base64::engine::general_purpose::STANDARD
                .decode(v.as_str().ok_or_else(|| bad("expected base64 string"))?)
                .map_err(|e| bad(&e.to_string()))?,
            mime_type: None,
        }),
        Node::Number => {
            let f = v
                .as_f64()
                .ok_or_else(|| bad("number cannot be represented exactly as finite f64"))?;
            if !f.is_finite() {
                return Err(bad("non-finite number"));
            }
            if v.as_i64().is_some_and(|i| f as i128 != i as i128)
                || v.as_u64().is_some_and(|u| f as u128 != u as u128)
            {
                return Err(bad("integer literal cannot be represented exactly as f64"));
            }
            SchemaValue::F64(f)
        }
        Node::Integer(k) => encode_int(*k, v, path)?,
        Node::Enum(c) => SchemaValue::Enum {
            case: c
                .iter()
                .position(|x| Some(x.as_str()) == v.as_str())
                .ok_or_else(|| bad("unknown enum case"))? as u32,
        },
        Node::Array(x) => SchemaValue::List {
            elements: v
                .as_array()
                .ok_or_else(|| bad("expected array"))?
                .iter()
                .enumerate()
                .map(|(i, v)| encode(x, v, defs, &format!("{path}/{i}")))
                .collect::<Result<_, _>>()?,
        },
        Node::Nullable(x) => SchemaValue::Option {
            inner: if v.is_null() {
                None
            } else {
                Some(Box::new(encode(x, v, defs, path)?))
            },
        },
        Node::Object { fields, extras } => {
            let o = v.as_object().ok_or_else(|| bad("expected object"))?;
            let mut out = Vec::new();
            for f in fields {
                let path = &format!(
                    "{path}/{}",
                    f.json_name.replace('~', "~0").replace('/', "~1")
                );
                match o.get(&f.json_name) {
                    Some(x) if f.required => out.push(encode(&f.node, x, defs, path)?),
                    Some(x) if f.node.nullable(defs) => out.push(SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::Record {
                            fields: vec![encode(&f.node, x, defs, path)?],
                        })),
                    }),
                    Some(x) => out.push(SchemaValue::Option {
                        inner: Some(Box::new(encode(&f.node, x, defs, path)?)),
                    }),
                    None if !f.required => out.push(SchemaValue::Option { inner: None }),
                    None => return Err(bad("missing required field")),
                }
            }
            if let Some(x) = extras {
                let known: BTreeSet<_> = fields.iter().map(|f| f.json_name.as_str()).collect();
                out.push(SchemaValue::Map {
                    entries: o
                        .iter()
                        .filter(|(k, _)| !known.contains(k.as_str()))
                        .map(|(k, v)| {
                            Ok((SchemaValue::String(k.clone()), encode(x, v, defs, path)?))
                        })
                        .collect::<Result<_, Error>>()?,
                });
            }
            SchemaValue::Record { fields: out }
        }
    })
}

fn decode(
    n: &Node,
    v: &SchemaValue,
    defs: &BTreeMap<String, Node>,
    path: &str,
) -> Result<Value, Error> {
    let bad = |r: &str| Error::Value {
        path: path.into(),
        reason: r.into(),
    };
    Ok(match (n.resolve(defs), v) {
        (Node::Null, SchemaValue::Tuple { elements }) if elements.is_empty() => Value::Null,
        (Node::Tuple(nodes), SchemaValue::Tuple { elements }) if nodes.len() == elements.len() => {
            Value::Array(
                nodes
                    .iter()
                    .zip(elements)
                    .enumerate()
                    .map(|(i, (n, v))| decode(n, v, defs, &format!("{path}/{i}")))
                    .collect::<Result<_, _>>()?,
            )
        }
        (Node::Union { field, cases }, SchemaValue::Union(value)) => {
            let (_, (tag, node)) = cases
                .iter()
                .enumerate()
                .find(|(i, _)| value.tag == format!("case-{i}"))
                .ok_or_else(|| bad("unknown union case"))?;
            let json = decode(node, &value.body, defs, path)?;
            if json.get(field).and_then(Value::as_str) != Some(tag.as_str()) {
                return Err(bad("union discriminator mismatch"));
            }
            json
        }
        (Node::Json, SchemaValue::String(s)) => {
            serde_json::from_str(s).map_err(|e| bad(&e.to_string()))?
        }
        (Node::Bool, SchemaValue::Bool(x)) => Value::Bool(*x),
        (Node::String, SchemaValue::String(x)) => Value::String(x.clone()),
        (Node::Binary, SchemaValue::Binary(x)) => {
            Value::String(base64::engine::general_purpose::STANDARD.encode(&x.bytes))
        }
        (Node::Number, SchemaValue::F64(x)) => serde_json::Number::from_f64(*x)
            .map(Value::Number)
            .ok_or_else(|| bad("non-finite f64"))?,
        (Node::Integer(kind), x) => {
            let json = int_json(x).ok_or_else(|| bad("integer representation mismatch"))?;
            if encode_int(*kind, &json, path)? != *x {
                return Err(bad("integer representation mismatch"));
            }
            json
        }
        (Node::Enum(c), SchemaValue::Enum { case }) => Value::String(
            c.get(*case as usize)
                .ok_or_else(|| bad("enum case out of range"))?
                .clone(),
        ),
        (Node::Array(x), SchemaValue::List { elements }) => Value::Array(
            elements
                .iter()
                .enumerate()
                .map(|(i, v)| decode(x, v, defs, &format!("{path}/{i}")))
                .collect::<Result<_, _>>()?,
        ),
        (Node::Nullable(_), SchemaValue::Option { inner: None }) => Value::Null,
        (Node::Nullable(x), SchemaValue::Option { inner: Some(v) }) => decode(x, v, defs, path)?,
        (Node::Object { fields, extras }, SchemaValue::Record { fields: values }) => {
            let expected = fields.len() + usize::from(extras.is_some());
            if values.len() != expected {
                return Err(bad("record field count mismatch"));
            }
            let mut o = Map::new();
            for (i, f) in fields.iter().enumerate() {
                let path = &format!(
                    "{path}/{}",
                    f.json_name.replace('~', "~0").replace('/', "~1")
                );
                if f.required {
                    o.insert(
                        f.json_name.clone(),
                        decode(&f.node, &values[i], defs, path)?,
                    );
                } else if let SchemaValue::Option { inner: Some(x) } = &values[i] {
                    let x = if f.node.nullable(defs) {
                        match x.as_ref() {
                            SchemaValue::Record { fields } if fields.len() == 1 => &fields[0],
                            _ => return Err(bad("nullable presence wrapper mismatch")),
                        }
                    } else {
                        x
                    };
                    o.insert(f.json_name.clone(), decode(&f.node, x, defs, path)?);
                } else if !matches!(values[i], SchemaValue::Option { inner: None }) {
                    return Err(bad("optional field representation mismatch"));
                }
            }
            if let Some(x) = extras {
                match &values[fields.len()] {
                    SchemaValue::Map { entries } => {
                        for (k, v) in entries {
                            let SchemaValue::String(k) = k else {
                                return Err(bad("extra property key is not string"));
                            };
                            if fields.iter().any(|f| f.json_name == *k) || o.contains_key(k) {
                                return Err(bad(
                                    "known or duplicate property supplied through extras",
                                ));
                            }
                            o.insert(k.clone(), decode(x, v, defs, path)?);
                        }
                    }
                    _ => return Err(bad("extras representation mismatch")),
                }
            }
            Value::Object(o)
        }
        _ => return Err(bad("schema value kind mismatch")),
    })
}

fn encode_int(k: IntKind, v: &Value, path: &str) -> Result<SchemaValue, Error> {
    let bad = || Error::Value {
        path: path.into(),
        reason: "integer is nonintegral or outside projected range".into(),
    };
    let i = v.as_i64();
    let u = v.as_u64();
    macro_rules! signed {
        ($t:ty,$c:ident) => {{
            let x = i
                .or_else(|| {
                    v.as_f64()
                        .filter(|_| v.is_f64())
                        .filter(|x| {
                            x.fract() == 0.0 && *x >= i64::MIN as f64 && *x < -(i64::MIN as f64)
                        })
                        .map(|x| x as i64)
                })
                .ok_or_else(bad)?;
            SchemaValue::$c(<$t>::try_from(x).map_err(|_| bad())?)
        }};
    }
    macro_rules! unsigned {
        ($t:ty,$c:ident) => {{
            let x = u
                .or_else(|| {
                    v.as_f64()
                        .filter(|_| v.is_f64())
                        .filter(|x| x.fract() == 0.0 && *x >= 0.0 && *x < u64::MAX as f64)
                        .map(|x| x as u64)
                })
                .ok_or_else(bad)?;
            SchemaValue::$c(<$t>::try_from(x).map_err(|_| bad())?)
        }};
    }
    Ok(match k {
        IntKind::S8 => signed!(i8, S8),
        IntKind::S16 => signed!(i16, S16),
        IntKind::S32 => signed!(i32, S32),
        IntKind::S64 => signed!(i64, S64),
        IntKind::U8 => unsigned!(u8, U8),
        IntKind::U16 => unsigned!(u16, U16),
        IntKind::U32 => unsigned!(u32, U32),
        IntKind::U64 => unsigned!(u64, U64),
    })
}
fn int_json(v: &SchemaValue) -> Option<Value> {
    Some(Value::Number(match v {
        SchemaValue::S8(x) => (*x).into(),
        SchemaValue::S16(x) => (*x).into(),
        SchemaValue::S32(x) => (*x).into(),
        SchemaValue::S64(x) => (*x).into(),
        SchemaValue::U8(x) => (*x).into(),
        SchemaValue::U16(x) => (*x).into(),
        SchemaValue::U32(x) => (*x).into(),
        SchemaValue::U64(x) => (*x).into(),
        _ => return None,
    }))
}

fn integer_kind(s: &Value, path: &str) -> Result<IntKind, Error> {
    let mut min = None;
    let mut max = None;
    for (key, lower, exclusive) in [
        ("minimum", true, false),
        ("maximum", false, false),
        ("exclusiveMinimum", true, true),
        ("exclusiveMaximum", false, true),
    ] {
        let Some(v) = s.get(key) else {
            continue;
        };
        let bound = if let Some(i) = v.as_i64() {
            i as i128
        } else if let Some(u) = v.as_u64() {
            u as i128
        } else {
            let f = v
                .as_f64()
                .ok_or_else(|| schema(path, "invalid numeric bound"))?;
            let rounded = if lower {
                if exclusive { f.floor() } else { f.ceil() }
            } else if exclusive {
                f.ceil()
            } else {
                f.floor()
            };
            rounded.clamp(i64::MIN as f64, u64::MAX as f64) as i128
        };
        let bound = bound
            + if exclusive {
                if lower { 1 } else { -1 }
            } else {
                0
            };
        if lower {
            min = Some(min.map_or(bound, |n: i128| n.max(bound)));
        } else {
            max = Some(max.map_or(bound, |n: i128| n.min(bound)));
        }
    }
    if min.zip(max).is_some_and(|(a, b)| a > b) {
        return Err(schema(path, "integer interval is empty"));
    }
    Ok(if min.is_some_and(|m| m >= 0) {
        let m = max.unwrap_or(u64::MAX as i128);
        if m <= u8::MAX as i128 {
            IntKind::U8
        } else if m <= u16::MAX as i128 {
            IntKind::U16
        } else if m <= u32::MAX as i128 {
            IntKind::U32
        } else {
            IntKind::U64
        }
    } else {
        let lo = min.unwrap_or(i64::MIN as i128);
        let hi = max.unwrap_or(i64::MAX as i128);
        if lo >= i8::MIN as i128 && hi <= i8::MAX as i128 {
            IntKind::S8
        } else if lo >= i16::MIN as i128 && hi <= i16::MAX as i128 {
            IntKind::S16
        } else if lo >= i32::MIN as i128 && hi <= i32::MAX as i128 {
            IntKind::S32
        } else {
            IntKind::S64
        }
    })
}
fn schema(path: &str, reason: &str) -> Error {
    Error::Schema {
        path: path.into(),
        reason: reason.into(),
    }
}
fn ref_id(r: &str) -> String {
    format!(
        "mcp.json-schema.{}",
        r.bytes().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn pointer<'a>(root: &'a Value, r: &str) -> Option<&'a Value> {
    let pointer = urlencoding::decode(r.strip_prefix('#')?).ok()?;
    root.pointer(&pointer)
}

// Follow validation-only schema edges too: reference depth is not JSON nesting depth.
fn check_validation_depth(
    value: &Value,
    source: &Value,
    depth: usize,
    nodes: &mut usize,
    active: &mut BTreeSet<*const Value>,
) -> Result<(), Error> {
    if !active.insert(value) {
        return Ok(());
    }
    *nodes = nodes
        .checked_sub(1)
        .ok_or_else(|| Error::Limit("validation schema nodes".into()))?;
    let mut visit = |value: &Value| {
        check_validation_depth(
            value,
            source,
            depth
                .checked_sub(1)
                .ok_or_else(|| Error::Limit("validation schema reference depth".into()))?,
            nodes,
            active,
        )
    };
    if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        visit(
            pointer(source, reference)
                .ok_or_else(|| schema(reference, "unresolved validation reference"))?,
        )?;
    }
    for (key, value) in value.as_object().into_iter().flatten() {
        match key.as_str() {
            "properties" | "patternProperties" | "dependentSchemas" => {
                for value in value.as_object().into_iter().flat_map(|o| o.values()) {
                    visit(value)?;
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                for value in value.as_array().into_iter().flatten() {
                    visit(value)?;
                }
            }
            "items"
            | "contains"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => visit(value)?,
            _ => {}
        }
    }
    active.remove(&(value as *const Value));
    Ok(())
}

fn reject_ref_machinery(v: &Value, path: &str) -> Result<(), Error> {
    let Some(o) = v.as_object() else {
        return Ok(());
    };
    if let Some(dialect) = o.get("$schema")
        && dialect.as_str() != Some("https://json-schema.org/draft/2020-12/schema")
    {
        return Err(schema(path, "only JSON Schema 2020-12 is supported"));
    }
    for key in [
        "$dynamicRef",
        "$dynamicAnchor",
        "$recursiveRef",
        "$recursiveAnchor",
        "$vocabulary",
    ] {
        if o.contains_key(key) {
            return Err(schema(path, &format!("unsupported schema keyword {key}")));
        }
    }
    if o.contains_key("$id") && !path.is_empty() {
        return Err(schema(path, "nested $id changes local reference scope"));
    }
    if let Some(reference) = o.get("$ref").and_then(Value::as_str)
        && reference
            .strip_prefix('#')
            .and_then(|r| urlencoding::decode(r).ok())
            .is_none_or(|p| !p.is_empty() && !p.starts_with('/'))
    {
        return Err(schema(
            path,
            "only document-local JSON pointer references are supported",
        ));
    }
    for (key, value) in o {
        match key.as_str() {
            "$defs" | "properties" | "patternProperties" | "dependentSchemas" => {
                if let Some(map) = value.as_object() {
                    for (name, child) in map {
                        reject_ref_machinery(child, &format!("{path}/{key}/{}", escape(name)))?;
                    }
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(items) = value.as_array() {
                    for (i, child) in items.iter().enumerate() {
                        reject_ref_machinery(child, &format!("{path}/{key}/{i}"))?;
                    }
                }
            }
            "items"
            | "contains"
            | "additionalProperties"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => {
                reject_ref_machinery(value, &format!("{path}/{key}"))?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn check_value(value: &SchemaValue, limits: Limits) -> Result<(), Error> {
    // Presence wrappers and discriminators add up to three representation levels per JSON level.
    let depth = limits
        .instance_depth
        .saturating_mul(3)
        .saturating_add(limits.schema_depth);
    let mut pending = vec![(value, 0)];
    let mut nodes = limits.instance_bytes.saturating_add(limits.schema_nodes);
    let mut bytes = limits.instance_bytes;
    while let Some((value, level)) = pending.pop() {
        if level > depth || nodes == 0 {
            return Err(Error::Limit("typed instance depth or nodes".into()));
        }
        nodes -= 1;
        let size = match value {
            SchemaValue::String(s) => s.len(),
            SchemaValue::Binary(b) => b.bytes.len(),
            SchemaValue::Record { fields }
            | SchemaValue::Tuple { elements: fields }
            | SchemaValue::List { elements: fields } => {
                if fields.len() > nodes {
                    return Err(Error::Limit("typed instance nodes".into()));
                }
                pending.extend(fields.iter().map(|v| (v, level + 1)));
                0
            }
            SchemaValue::Map { entries } => {
                if entries.len() > nodes / 2 {
                    return Err(Error::Limit("typed instance nodes".into()));
                }
                for (key, value) in entries {
                    pending.extend([(key, level + 1), (value, level + 1)]);
                }
                0
            }
            SchemaValue::Option { inner: Some(value) } => {
                pending.push((value, level + 1));
                0
            }
            SchemaValue::Union(value) => {
                pending.push((&value.body, level + 1));
                0
            }
            // Other composite/capability kinds are rejected by the projection decoder.
            _ => 0,
        };
        bytes = bytes
            .checked_sub(size)
            .ok_or_else(|| Error::Limit("typed instance bytes".into()))?;
    }
    Ok(())
}

fn check_tree(
    v: &Value,
    max_bytes: usize,
    max_nodes: usize,
    max_depth: usize,
    label: &str,
) -> Result<(), Error> {
    crate::limits::check(v, max_bytes, max_nodes, max_depth)
        .map_err(|e| Error::Limit(format!("{label} {e:?}")))
}

#[cfg(test)]
mod tests;
