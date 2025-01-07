use anyhow::Result;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{Value, ValueAndType};
use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
use poem_openapi::{types::{IsObjectType, ParseFromJSON, ToJSON, Type}, Object, Union};
use rib::{
    ArmPattern, Expr, FunctionTypeRegistry, InferredType,
};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use once_cell::sync::Lazy;

// Global string interner for static strings
static STRING_INTERNER: Lazy<Mutex<HashSet<&'static str>>> = Lazy::new(|| Mutex::new(HashSet::new()));

pub struct RibConverter {
    openapi_mode: bool,
    in_openapi_operation: bool,
    type_registry: Option<FunctionTypeRegistry>,
    current_field_name: Option<String>,
}

// Define our Poem OpenAPI types
#[derive(Object, Debug, Clone)]
struct RibBool {
    value: bool,
}

#[derive(Object, Debug, Clone)]
struct RibStr {
    value: String,
}

#[derive(Object, Debug, Clone)]
struct RibU32 {
    value: u32,
}

#[derive(Object, Debug, Clone)]
struct RibS32 {
    value: i32,
}

#[derive(Object, Debug, Clone)]
struct RibU64 {
    value: u64,
}

#[derive(Object, Debug, Clone)]
struct RibS64 {
    value: i64,
}

#[derive(Object, Debug, Clone)]
struct RibF32 {
    value: f32,
}

#[derive(Object, Debug, Clone)]
struct RibF64 {
    value: f64,
}

#[derive(Union, Debug, Clone)]
#[oai(discriminator_name = "type")]
enum RibValue {
    #[oai(mapping = "bool")]
    Bool(RibBool),
    #[oai(mapping = "str")]
    Str(RibStr),
    #[oai(mapping = "u32")]
    U32(RibU32),
    #[oai(mapping = "s32")]
    S32(RibS32),
    #[oai(mapping = "u64")]
    U64(RibU64),
    #[oai(mapping = "s64")]
    S64(RibS64),
    #[oai(mapping = "f32")]
    F32(RibF32),
    #[oai(mapping = "f64")]
    F64(RibF64),
}

#[derive(Object, Debug, Clone)]
struct RibList<T: Type + ParseFromJSON + ToJSON> {
    items: Vec<T>,
}

#[derive(Object, Debug, Clone)]
struct RibRecord {
    fields: HashMap<String, RibValue>,
}

#[derive(Object, Debug, Clone)]
struct RibTuple {
    items: Vec<RibValue>,
}

#[derive(Union, Debug, Clone)]
#[oai(discriminator_name = "type")]
enum RibOption<T: Type + ParseFromJSON + ToJSON + IsObjectType> {
    #[oai(mapping = "some")]
    Some(T),
    #[oai(mapping = "none")]
    None(RibEmpty),
}

#[derive(Object, Debug, Clone)]
struct RibEmpty {}

#[derive(Union, Debug, Clone)]
#[oai(discriminator_name = "type")]
enum RibResult<T: Type + ParseFromJSON + ToJSON + IsObjectType, E: Type + ParseFromJSON + ToJSON + IsObjectType> {
    #[oai(mapping = "ok")]
    Ok(T),
    #[oai(mapping = "error")]
    Error(E),
}

#[derive(Union, Debug, Clone)]
#[oai(discriminator_name = "type")]
enum RibVariant {
    #[oai(mapping = "variant")]
    Variant(RibValue),
}

#[derive(Object, Debug, Clone)]
struct RibEnum {
    #[oai(validator(pattern = "enum_values"))]
    value: String,
}

#[derive(Object, Debug, Clone)]
struct RibFlags {
    #[oai(validator(pattern = "flag_values"))]
    value: String,
}

impl RibConverter {
    pub fn new_openapi() -> Self {
        Self {
            openapi_mode: true,
            in_openapi_operation: false,
            type_registry: None,
            current_field_name: None,
        }
    }

    pub fn new_wit() -> Self {
        Self {
            openapi_mode: false,
            in_openapi_operation: false,
            type_registry: None,
            current_field_name: None,
        }
    }

    pub fn with_type_registry(mut self, registry: FunctionTypeRegistry) -> Self {
        self.type_registry = Some(registry);
        self
    }

    // For backward compatibility
    pub fn new() -> Self {
        Self::new_openapi()
    }

    pub fn set_in_openapi_operation(&mut self, in_openapi: bool) {
        self.in_openapi_operation = in_openapi;
    }

    fn store_string<S: AsRef<str>>(&self, s: S) -> &'static str {
        let s = s.as_ref();
        let mut interner = STRING_INTERNER.lock().unwrap();
        
        // Check if we already have this string
        if let Some(existing) = interner.get(s) {
            return existing;
        }
        
        // Allocate a new static string
        let leaked = Box::leak(s.to_owned().into_boxed_str());
        interner.insert(leaked);
        leaked
    }

    pub fn convert_type(&mut self, typ: &AnalysedType, _registry: &Registry) -> Result<MetaSchemaRef, String> {
        let schema_ref = match typ {
            AnalysedType::Bool(_) => {
                let mut schema = MetaSchema::new("boolean");
                schema.ty = "boolean";
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::U8(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                schema.minimum = Some(0.0);
                schema.maximum = Some(255.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::S8(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                schema.minimum = Some(-128.0);
                schema.maximum = Some(127.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::U16(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                schema.minimum = Some(0.0);
                schema.maximum = Some(65535.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::S16(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                schema.minimum = Some(-32768.0);
                schema.maximum = Some(32767.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::U32(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                schema.minimum = Some(0.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::S32(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int32");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::U64(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int64");
                schema.minimum = Some(0.0);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::S64(_) => {
                let mut schema = MetaSchema::new("integer");
                schema.format = Some("int64");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::F32(_) => {
                let mut schema = MetaSchema::new("number");
                schema.format = Some("float");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::F64(_) => {
                let mut schema = MetaSchema::new("number");
                schema.format = Some("double");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Chr(_) => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                schema.min_length = Some(1);
                schema.max_length = Some(1);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Str(_) => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                
                // Add format if we're in OpenAPI mode and have a field name
                if self.openapi_mode {
                    if let Some(field_name) = &self.current_field_name {
                        match field_name.as_str() {
                            "email" => schema.format = Some("email"),
                            "date" => schema.format = Some("date"),
                            "uuid" => schema.format = Some("uuid"),
                            _ => {}
                        }
                    }
                }
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Enum(enum_type) => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                schema.enum_items = enum_type.cases.iter()
                    .map(|case| serde_json::Value::String(case.clone()))
                    .collect();
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::List(list_type) => {
                let items_schema = self.convert_type(&list_type.inner, _registry)?;
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                schema.items = Some(Box::new(items_schema));
                
                // Add array validation
                schema.min_items = Some(0);
                schema.unique_items = Some(false);
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Tuple(tuple_type) => {
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                
                // Create a oneOf schema for tuple items
                let mut items_schema = MetaSchema::new("object");
                let mut one_of = Vec::new();
                
                // Convert each tuple item type
                for item_type in &tuple_type.items {
                    let item_schema = self.convert_type(item_type, _registry)?;
                    one_of.push(item_schema);
                }
                
                items_schema.one_of = one_of;
                schema.items = Some(Box::new(MetaSchemaRef::Inline(Box::new(items_schema))));
                
                // Set fixed size constraints
                let size = tuple_type.items.len();
                schema.min_items = Some(size);
                schema.max_items = Some(size);
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Record(record_type) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut properties = Vec::new();
                let mut required = Vec::new();
                
                for field in &record_type.fields {
                    self.current_field_name = Some(field.name.clone());
                    let field_schema = self.convert_type(&field.typ, _registry)?;
                    let static_name = self.store_string(&field.name);
                    properties.push((static_name, field_schema));
                    required.push(static_name);
                }
                self.current_field_name = None;
                
                schema.properties = properties;
                schema.required = required;
                
                // Set additional_properties to false to disallow any extra fields
                schema.additional_properties = Some(Box::new(MetaSchemaRef::Inline(Box::new(MetaSchema {
                    rust_typename: None,
                    ty: "boolean",
                    format: None,
                    title: None,
                    description: None,
                    max_properties: None,
                    min_properties: None,
                    read_only: false,
                    write_only: false,
                    default: None,
                    properties: Vec::new(),
                    required: Vec::new(),
                    items: None,
                    additional_properties: None,
                    one_of: Vec::new(),
                    all_of: Vec::new(),
                    any_of: Vec::new(),
                    nullable: false,
                    discriminator: None,
                    enum_items: Vec::new(),
                    min_length: None,
                    max_length: None,
                    pattern: None,
                    minimum: None,
                    maximum: None,
                    exclusive_minimum: None,
                    exclusive_maximum: None,
                    multiple_of: None,
                    unique_items: None,
                    deprecated: false,
                    example: None,
                    external_docs: None,
                    min_items: None,
                    max_items: None,
                }))));
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Variant(variant_type) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                schema.required = vec!["type"];
                let mut properties = Vec::new();
                
                // Add type discriminator property
                let mut type_schema = MetaSchema::new("string");
                type_schema.ty = "string";
                type_schema.enum_items = variant_type.cases.iter()
                    .map(|case| serde_json::Value::String(case.name.clone()))
                    .collect();
                properties.push(("type", MetaSchemaRef::Inline(Box::new(type_schema))));
                
                // Add value property if any case has a type
                if variant_type.cases.iter().any(|case| case.typ.is_some()) {
                    let mut value_schema = MetaSchema::new("object");
                    value_schema.ty = "object";
                    let mut one_of = Vec::new();
                    
                    for case in &variant_type.cases {
                        if let Some(case_type) = &case.typ {
                            let case_schema = self.convert_type(case_type, _registry)?;
                            one_of.push(case_schema);
                        }
                    }
                    
                    if !one_of.is_empty() {
                        value_schema.one_of = one_of;
                        properties.push(("value", MetaSchemaRef::Inline(Box::new(value_schema))));
                    }
                }
                
                schema.properties = properties;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Result(result_type) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                schema.required = vec!["type"];
                let mut properties = Vec::new();
                
                // Add type discriminator property
                let mut type_schema = MetaSchema::new("string");
                type_schema.ty = "string";
                type_schema.enum_items = vec![
                    serde_json::Value::String("ok".to_string()),
                    serde_json::Value::String("error".to_string()),
                ];
                properties.push(("type", MetaSchemaRef::Inline(Box::new(type_schema))));
                
                // Add value property if either ok or error type is present
                let mut has_value = false;
                let mut value_schema = MetaSchema::new("object");
                value_schema.ty = "object";
                let mut one_of = Vec::new();
                
                if let Some(ok_type) = &result_type.ok {
                    let ok_schema = self.convert_type(ok_type, _registry)?;
                    one_of.push(ok_schema);
                    has_value = true;
                }
                
                if let Some(err_type) = &result_type.err {
                    let err_schema = self.convert_type(err_type, _registry)?;
                    one_of.push(err_schema);
                    has_value = true;
                }
                
                if has_value {
                    value_schema.one_of = one_of;
                    properties.push(("value", MetaSchemaRef::Inline(Box::new(value_schema))));
                }
                
                schema.properties = properties;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            AnalysedType::Option(option_type) => {
                // Convert the inner type
                let mut inner_schema = self.convert_type(&option_type.inner, _registry)?;
                
                // Make it nullable
                match &mut inner_schema {
                    MetaSchemaRef::Inline(schema) => {
                        schema.nullable = true;
                    },
                    MetaSchemaRef::Reference(_) => {
                        // For referenced schemas, we need to wrap it in a new schema
                        let mut wrapper = MetaSchema::new("object");
                        wrapper.one_of = vec![
                            MetaSchemaRef::Inline(Box::new(MetaSchema {
                                rust_typename: None,
                                ty: "null",
                                format: None,
                                title: None,
                                description: None,
                                max_properties: None,
                                min_properties: None,
                                read_only: false,
                                write_only: false,
                                default: None,
                                properties: Vec::new(),
                                required: Vec::new(),
                                items: None,
                                additional_properties: None,
                                one_of: Vec::new(),
                                all_of: Vec::new(),
                                any_of: Vec::new(),
                                nullable: false,
                                discriminator: None,
                                enum_items: Vec::new(),
                                min_length: None,
                                max_length: None,
                                pattern: None,
                                minimum: None,
                                maximum: None,
                                exclusive_minimum: None,
                                exclusive_maximum: None,
                                multiple_of: None,
                                unique_items: None,
                                deprecated: false,
                                example: None,
                                external_docs: None,
                                min_items: None,
                                max_items: None,
                            })),
                            inner_schema.clone(),
                        ];
                        inner_schema = MetaSchemaRef::Inline(Box::new(wrapper));
                    }
                }
                
                Ok(inner_schema)
            },
            _ => Err("Unsupported type".to_string()),
        }?;

        Ok(schema_ref)
    }

    pub fn convert_value(&mut self, value: &ValueAndType) -> Result<serde_json::Value, String> {
        // Try using wasm-rpc's conversion first
        if !self.in_openapi_operation {
            return self.convert_wit_value(value);
        }
        
        // Fall back to OpenAPI mode conversion for OpenAPI operations
        match &value.value {
            Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
            Value::String(s) => Ok(serde_json::Value::String(s.clone())),
            Value::Char(c) => {
                match char::from_u32(*c as u32) {
                    Some(ch) => Ok(serde_json::Value::String(ch.to_string())),
                    None => Err(format!("Invalid Unicode scalar value: {}", c))
                }
            },
            
            // Unsigned integers
            Value::U8(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::U16(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::U32(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::U64(n) => Ok(serde_json::Value::Number((*n).into())),
            
            // Signed integers
            Value::S8(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::S16(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::S32(n) => Ok(serde_json::Value::Number((*n).into())),
            Value::S64(n) => Ok(serde_json::Value::Number((*n).into())),
            
            // Floating point
            Value::F32(n) => Ok(serde_json::Value::Number(serde_json::Number::from_f64(*n as f64).unwrap())),
            Value::F64(n) => Ok(serde_json::Value::Number(serde_json::Number::from_f64(*n).unwrap())),
            
            Value::List(items) => {
                let mut values = Vec::new();
                if let AnalysedType::List(list_type) = &value.typ {
                    for item in items {
                        match item {
                            Value::Option(None) => {
                                values.push(serde_json::Value::Null);
                            },
                            _ => {
                                values.push(self.convert_value(&ValueAndType {
                                    value: item.clone(),
                                    typ: list_type.inner.as_ref().clone(),
                                })?);
                            }
                        }
                    }
                };
                Ok(serde_json::Value::Array(values))
            },
            
            Value::Record(fields) => {
                let mut map = serde_json::Map::new();
                if let AnalysedType::Record(record_type) = &value.typ {
                    for (field, field_type) in fields.iter().zip(record_type.fields.iter()) {
                        let field_value = self.convert_value(&ValueAndType {
                            value: field.clone(),
                            typ: field_type.typ.clone(),
                        })?;
                        map.insert(field_type.name.clone(), field_value);
                    }
                };
                Ok(serde_json::Value::Object(map))
            },
            
            Value::Option(opt) => match opt {
                Some(inner) => {
                    if let AnalysedType::Option(opt_type) = &value.typ {
                        self.convert_value(&ValueAndType {
                            value: *inner.clone(),
                            typ: opt_type.inner.as_ref().clone(),
                        })
                    } else {
                        Ok(serde_json::Value::Null)
                    }
                },
                None => Ok(serde_json::Value::Null),
            },
            
            Value::Result(result) => {
                match result {
                    Ok(ok) => {
                        let mut map = serde_json::Map::new();
                        map.insert("type".to_string(), serde_json::Value::String("ok".to_string()));
                        if let Some(inner) = ok {
                            if let AnalysedType::Result(result_type) = &value.typ {
                                if let Some(ok_type) = &result_type.ok {
                                    let inner_value = self.convert_value(&ValueAndType {
                                        value: *inner.clone(),
                                        typ: ok_type.as_ref().clone(),
                                    })?;
                                    map.insert("value".to_string(), inner_value);
                                }
                            }
                        } else {
                            map.insert("value".to_string(), serde_json::Value::Null);
                        }
                        Ok(serde_json::Value::Object(map))
                    },
                    Err(err) => {
                        let mut map = serde_json::Map::new();
                        map.insert("type".to_string(), serde_json::Value::String("error".to_string()));
                        if let Some(inner) = err {
                            if let AnalysedType::Result(result_type) = &value.typ {
                                if let Some(err_type) = &result_type.err {
                                    let inner_value = self.convert_value(&ValueAndType {
                                        value: *inner.clone(),
                                        typ: err_type.as_ref().clone(),
                                    })?;
                                    map.insert("value".to_string(), inner_value);
                                }
                            }
                        } else {
                            map.insert("value".to_string(), serde_json::Value::Null);
                        }
                        Ok(serde_json::Value::Object(map))
                    }
                }
            },
            
            Value::Variant { case_idx, case_value } => {
                let result = if let AnalysedType::Variant(variant) = &value.typ {
                    let case = &variant.cases[*case_idx as usize];
                    let mut map = serde_json::Map::new();
                    map.insert("type".to_string(), serde_json::Value::String(case.name.clone()));
                    
                    if let Some(inner) = case_value {
                        if let Some(case_type) = &case.typ {
                            let inner_value = self.convert_value(&ValueAndType {
                                value: *inner.clone(),
                                typ: case_type.clone(),
                            })?;
                            map.insert("value".to_string(), inner_value);
                        }
                    }
                    
                    Ok(serde_json::Value::Object(map))
                } else {
                    Ok(serde_json::Value::Null)
                };
                result
            },
            
            Value::Enum(idx) => {
                if let AnalysedType::Enum(enum_type) = &value.typ {
                    Ok(serde_json::Value::String(enum_type.cases[*idx as usize].clone()))
                } else {
                    Ok(serde_json::Value::Null)
                }
            },
            
            Value::Flags(flags) => {
                // Return an array of strings, each string is a flag
                let arr = flags.iter().map(|s| serde_json::Value::String(s.to_string())).collect::<Vec<_>>();
                Ok(serde_json::Value::Array(arr))
            },
            
            Value::Handle { uri, resource_id } => {
                Ok(serde_json::Value::String(format!("{}:{}", uri, resource_id)))
            },

            // Catch-all for any future variants
            _ => Ok(serde_json::Value::Null),
        }
    }

    pub fn convert_wit_value(&mut self, value: &ValueAndType) -> Result<serde_json::Value, String> {
        // Convert ValueAndType to TypeAnnotatedValue first
        let type_annotated: TypeAnnotatedValue = value.try_into()
            .map_err(|e: Vec<String>| e.join(", "))?;
        
        // Then convert to JSON
        Ok(type_annotated.to_json_value())
    }

    pub fn parse_wit_value(json: &serde_json::Value, typ: &AnalysedType) -> Result<ValueAndType, String> {
        // Use wasm-rpc's parsing by default
        let type_annotated = TypeAnnotatedValue::parse_with_type(json, typ)
            .map_err(|e| e.join(", "))?;
            
        // Convert back to ValueAndType
        ValueAndType::try_from(type_annotated)
            .map_err(|e| format!("Failed to convert from TypeAnnotatedValue: {}", e))
    }

    pub fn parse_openapi_value(json: &serde_json::Value, typ: &AnalysedType) -> Result<ValueAndType, String> {
        // First try using WIT parsing
        if let Ok(value) = Self::parse_wit_value(json, typ) {
            return Ok(value);
        }

        // Fall back to OpenAPI-specific parsing if needed
        match typ {
            // Add OpenAPI-specific parsing logic here if needed
            _ => Self::parse_wit_value(json, typ)
        }
    }

    pub fn convert_inferred_type(&mut self, typ: &InferredType, registry: &Registry) -> Result<MetaSchemaRef, String> {
        match typ {
            InferredType::Bool => {
                let mut schema = MetaSchema::new("boolean");
                schema.ty = "boolean";
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::S8 | InferredType::U8 | 
            InferredType::S16 | InferredType::U16 |
            InferredType::S32 | InferredType::U32 => {
                let mut schema = MetaSchema::new("integer");
                schema.ty = "integer";
                schema.format = Some("int32");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::S64 | InferredType::U64 => {
                let mut schema = MetaSchema::new("integer");
                schema.ty = "integer";
                schema.format = Some("int64");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Chr => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                schema.min_length = Some(1);
                schema.max_length = Some(1);
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Str => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                
                // Add format if we're in OpenAPI mode and have a field name
                if self.openapi_mode {
                    if let Some(field_name) = &self.current_field_name {
                        match field_name.as_str() {
                            "email" => schema.format = Some("email"),
                            "date" => schema.format = Some("date"),
                            "uuid" => schema.format = Some("uuid"),
                            _ => {}
                        }
                    }
                }
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::List(list_type) => {
                let items_schema = self.convert_inferred_type(&list_type, registry)?;
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                schema.items = Some(Box::new(items_schema));
                
                // Add array validation
                schema.min_items = Some(0);
                schema.unique_items = Some(false);
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Record(fields) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut properties = Vec::new();
                let mut required = Vec::new();
                
                for (field_name, field_type) in fields {
                    self.current_field_name = Some(field_name.clone());
                    let field_schema = self.convert_inferred_type(field_type, registry)?;
                    let static_name = self.store_string(field_name);
                    properties.push((static_name, field_schema));
                    required.push(static_name);
                }
                self.current_field_name = None;
                
                schema.properties = properties;
                schema.required = required;
                
                // Create a simple boolean schema for additional properties
                let mut additional_props = MetaSchema::new("boolean");
                additional_props.ty = "boolean";
                schema.additional_properties = Some(Box::new(MetaSchemaRef::Inline(Box::new(additional_props))));
                
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Variant(variant_cases) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut one_of = Vec::new();
                
                for (case_name, case_type) in variant_cases {
                    let mut case_schema = MetaSchema::new("object");
                    case_schema.ty = "object";
                    let mut case_properties = Vec::new();
                    
                    if let Some(case_type) = case_type {
                        let inner_schema = self.convert_inferred_type(&case_type, registry)?;
                        let static_name = self.store_string(case_name);
                        case_properties.push((static_name, inner_schema));
                        case_schema.required = vec![static_name];
                    } else {
                        let static_name = self.store_string(case_name);
                        let null_schema = MetaSchema::new("null");
                        case_properties.push((static_name, MetaSchemaRef::Inline(Box::new(null_schema))));
                        case_schema.required = vec![static_name];
                    }
                    
                    case_schema.properties = case_properties;
                    one_of.push(MetaSchemaRef::Inline(Box::new(case_schema)));
                }
                
                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Option(inner) => {
                let inner_schema = self.convert_inferred_type(inner, registry)?;
                let mut schema = MetaSchema::new("object");
                schema.nullable = true;
                schema.any_of = vec![inner_schema];
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Result { ok, error } => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut one_of = Vec::new();
                
                if let Some(ok_type) = ok {
                    let mut ok_schema = MetaSchema::new("object");
                    ok_schema.ty = "object";
                    let ok_inner_schema = self.convert_inferred_type(ok_type, registry)?;
                    ok_schema.properties = vec![("Ok", ok_inner_schema)];
                    ok_schema.required = vec!["Ok"];
                    one_of.push(MetaSchemaRef::Inline(Box::new(ok_schema)));
                }
                
                if let Some(err_type) = error {
                    let mut err_schema = MetaSchema::new("object");
                    err_schema.ty = "object";
                    let err_inner_schema = self.convert_inferred_type(err_type, registry)?;
                    err_schema.properties = vec![("Err", err_inner_schema)];
                    err_schema.required = vec!["Err"];
                    one_of.push(MetaSchemaRef::Inline(Box::new(err_schema)));
                }
                
                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Flags(flags) => {
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                let mut item_schema = MetaSchema::new("string");
                item_schema.ty = "string";
                // Create oneOf for the enum values
                let one_of: Vec<MetaSchemaRef> = flags.iter()
                    .map(|s| {
                        let mut value_schema = MetaSchema::new("string");
                        value_schema.ty = "string";
                        value_schema.title = Some(s.to_string());
                        MetaSchemaRef::Inline(Box::new(value_schema))
                    })
                    .collect();
                item_schema.one_of = one_of;
                schema.items = Some(Box::new(MetaSchemaRef::Inline(Box::new(item_schema))));
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Enum(enum_type) => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                // Create oneOf for the enum values
                let one_of: Vec<MetaSchemaRef> = enum_type.iter()
                    .map(|s| {
                        let mut value_schema = MetaSchema::new("string");
                        value_schema.ty = "string";
                        value_schema.title = Some(s.to_string());
                        MetaSchemaRef::Inline(Box::new(value_schema))
                    })
                    .collect();
                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Unknown => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Tuple(types) => {
                // For OpenAPI 3.0 compatibility (no prefixItems support), we use a oneOf that includes:
                // 1. A fixed-length array with the first type (for simpler array-like access)
                // 2. An object with indexed fields (for precise type information)
                let mut schema = MetaSchema::new("object");
                let mut one_of = Vec::new();

                // Approach 1: Array representation (simpler access)
                let mut array_schema = MetaSchema::new("array");
                array_schema.ty = "array";
                if let Some(first_type) = types.first() {
                    let first_schema = self.convert_inferred_type(first_type, registry)?;
                    array_schema.items = Some(Box::new(first_schema));
                }
                array_schema.min_items = Some(types.len());
                array_schema.max_items = Some(types.len());
                one_of.push(MetaSchemaRef::Inline(Box::new(array_schema)));

                // Approach 2: Object representation (precise types)
                let mut object_schema = MetaSchema::new("object");
                object_schema.ty = "object";
                let mut properties = Vec::new();
                let mut required = Vec::new();

                for (idx, typ) in types.iter().enumerate() {
                    let field_schema = self.convert_inferred_type(typ, registry)?;
                    let field_name = self.store_string(&format!("{}", idx));
                    properties.push((field_name, field_schema));
                    required.push(field_name);
                }

                object_schema.properties = properties;
                object_schema.required = required;
                object_schema.additional_properties = Some(Box::new(MetaSchemaRef::Inline(Box::new(MetaSchema {
                    rust_typename: None,
                    ty: "boolean",
                    format: None,
                    title: None,
                    description: None,
                    max_properties: None,
                    min_properties: None,
                    read_only: false,
                    write_only: false,
                    default: None,
                    properties: Vec::new(),
                    required: Vec::new(),
                    items: None,
                    additional_properties: None,
                    one_of: Vec::new(),
                    all_of: Vec::new(),
                    any_of: Vec::new(),
                    nullable: false,
                    discriminator: None,
                    enum_items: Vec::new(),
                    min_length: None,
                    max_length: None,
                    pattern: None,
                    minimum: None,
                    maximum: None,
                    exclusive_minimum: None,
                    exclusive_maximum: None,
                    multiple_of: None,
                    unique_items: None,
                    deprecated: false,
                    example: None,
                    external_docs: None,
                    min_items: None,
                    max_items: None,
                }))));
                one_of.push(MetaSchemaRef::Inline(Box::new(object_schema)));

                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Resource { resource_id, resource_mode } => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                let desc = format!("Resource ID: {}, Mode: {}", resource_id, resource_mode);
                schema.description = Some(self.store_string(&desc));
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::OneOf(types) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut one_of = Vec::new();
                for typ in types {
                    let type_schema = self.convert_inferred_type(typ, registry)?;
                    one_of.push(type_schema);
                }
                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::AllOf(types) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut all_of = Vec::new();
                for typ in types {
                    let type_schema = self.convert_inferred_type(typ, registry)?;
                    all_of.push(type_schema);
                }
                schema.all_of = all_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            InferredType::Sequence(types) => {
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                // Use the first type as the array item type
                if let Some(first_type) = types.first() {
                    let item_schema = self.convert_inferred_type(first_type, registry)?;
                    schema.items = Some(Box::new(item_schema));
                }
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },
            _ => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            }
        }
    }

    pub fn convert_expr(
        &mut self,
        expr: &Expr,
        registry: &Registry,
    ) -> Result<MetaSchemaRef, String> {
        match expr {
            // Handle string interpolation
            Expr::Concat(_parts, _) => {
                let schema = MetaSchema::new("string");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle code blocks
            Expr::ExprBlock(exprs, _) => {
                if exprs.is_empty() {
                    let schema = MetaSchema::new("null");
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                } else {
                    // Last expression determines the type
                    self.convert_expr(exprs.last().unwrap(), registry)
                }
            },

            // Handle complex records
            Expr::Record(fields, _) => {
                let mut schema = MetaSchema::new("object");
                let mut properties = Vec::new();
                let mut required = Vec::new();
                
                for (name, field_expr) in fields {
                    let field_schema = self.convert_expr(field_expr, registry)?;
                    let static_name = self.store_string(name);
                    properties.push((static_name, field_schema));
                    required.push(static_name);
                }
                
                schema.properties = properties;
                schema.required = required;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle tuples
            Expr::Tuple(exprs, _) => {
                let mut schema = MetaSchema::new("array");
                if let Some(first) = exprs.first() {
                    let item_schema = self.convert_expr(first, registry)?;
                    schema.items = Some(Box::new(item_schema));
                }
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle let bindings
            Expr::Let(_, _, expr, _) => {
                self.convert_expr(expr, registry)
            },

            // Handle field selection
            Expr::SelectField(expr, _, _) => {
                self.convert_expr(expr, registry)
            },

            // Handle index selection
            Expr::SelectIndex(expr, _, _) => {
                self.convert_expr(expr, registry)
            },

            // Handle sequences
            Expr::Sequence(exprs, _) => {
                if exprs.is_empty() {
                    let schema = MetaSchema::new("null");
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                } else {
                    // Last expression determines the type
                    self.convert_expr(exprs.last().unwrap(), registry)
                }
            },

            // Handle literals
            Expr::Literal(value, _) => {
                // Try to parse as number first
                if let Ok(_) = value.parse::<i64>() {
                    let mut schema = MetaSchema::new("integer");
                    schema.ty = "integer";
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                } else if let Ok(_) = value.parse::<f64>() {
                    let mut schema = MetaSchema::new("number");
                    schema.ty = "number";
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                } else if value == "true" || value == "false" {
                    let mut schema = MetaSchema::new("boolean");
                    schema.ty = "boolean";
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                } else {
                    let mut schema = MetaSchema::new("string");
                    schema.ty = "string";
                    Ok(MetaSchemaRef::Inline(Box::new(schema)))
                }
            },

            // Handle variables
            Expr::Identifier(_, inferred_type) => {
                self.convert_inferred_type(inferred_type, registry)
            },

            // Handle function calls
            Expr::Call(_, args, return_type) => {
                if args.is_empty() {
                    self.convert_inferred_type(return_type, registry)
                } else {
                    // Convert the last argument's type as it determines the return type
                    self.convert_expr(args.last().unwrap(), registry)
                }
            },

            // Handle pattern matching
            Expr::PatternMatch(expr, arms, _) => {
                let mut schema = MetaSchema::new("object");
                let mut one_of = Vec::new();

                // Convert the matched expression
                let match_schema = self.convert_expr(expr, registry)?;
                one_of.push(match_schema.clone());

                // Convert each arm's pattern
                for arm in arms {
                    match &arm.arm_pattern {
                        ArmPattern::Constructor(_, inner_patterns) => {
                            let mut inner_schema = MetaSchema::new("object");
                            inner_schema.ty = "object";
                            let mut properties = Vec::new();
                            
                            for (i, _) in inner_patterns.iter().enumerate() {
                                let field_schema = self.convert_expr(&Expr::Literal(format!("field_{}", i), InferredType::Unknown), registry)?;
                                let static_name = self.store_string(&format!("field_{}", i));
                                properties.push((static_name, field_schema));
                            }
                            
                            inner_schema.properties = properties;
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        },
                        ArmPattern::WildCard => {
                            let inner_schema = MetaSchema::new("object");
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        },
                        ArmPattern::Literal(expr) => {
                            let inner_schema = self.convert_expr(expr, registry)?;
                            one_of.push(inner_schema);
                        },
                        ArmPattern::As(name, _) => {
                            let mut inner_schema = MetaSchema::new("object");
                            inner_schema.ty = "object";
                            let static_name = self.store_string(name);
                            inner_schema.properties = vec![(static_name, match_schema.clone())];
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        },
                        ArmPattern::TupleConstructor(patterns) => {
                            let mut inner_schema = MetaSchema::new("array");
                            inner_schema.ty = "array";
                            if let Some(first) = patterns.first() {
                                match first {
                                    ArmPattern::Literal(expr) => {
                                        let item_schema = self.convert_expr(expr, registry)?;
                                        inner_schema.items = Some(Box::new(item_schema));
                                    },
                                    _ => {
                                        let item_schema = MetaSchema::new("object");
                                        inner_schema.items = Some(Box::new(MetaSchemaRef::Inline(Box::new(item_schema))));
                                    }
                                }
                            }
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        },
                        ArmPattern::RecordConstructor(fields) => {
                            let mut inner_schema = MetaSchema::new("object");
                            inner_schema.ty = "object";
                            let mut properties = Vec::new();
                            
                            for (name, pattern) in fields {
                                match pattern {
                                    ArmPattern::Literal(expr) => {
                                        let field_schema = self.convert_expr(expr, registry)?;
                                        let static_name = self.store_string(name);
                                        properties.push((static_name, field_schema));
                                    },
                                    _ => {
                                        let field_schema = MetaSchema::new("object");
                                        let static_name = self.store_string(name);
                                        properties.push((static_name, MetaSchemaRef::Inline(Box::new(field_schema))));
                                    }
                                }
                            }
                            
                            inner_schema.properties = properties;
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        },
                        ArmPattern::ListConstructor(patterns) => {
                            let mut inner_schema = MetaSchema::new("array");
                            inner_schema.ty = "array";
                            if let Some(first) = patterns.first() {
                                match first {
                                    ArmPattern::Literal(expr) => {
                                        let item_schema = self.convert_expr(expr, registry)?;
                                        inner_schema.items = Some(Box::new(item_schema));
                                    },
                                    _ => {
                                        let item_schema = MetaSchema::new("object");
                                        inner_schema.items = Some(Box::new(MetaSchemaRef::Inline(Box::new(item_schema))));
                                    }
                                }
                            }
                            one_of.push(MetaSchemaRef::Inline(Box::new(inner_schema)));
                        }
                    }
                }

                schema.one_of = one_of;
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle comparison operations
            Expr::EqualTo(_, _, _) |
            Expr::LessThan(_, _, _) |
            Expr::LessThanOrEqualTo(_, _, _) |
            Expr::GreaterThan(_, _, _) |
            Expr::GreaterThanOrEqualTo(_, _, _) |
            Expr::And(_, _, _) |
            Expr::Or(_, _, _) |
            Expr::Not(_, _) => {
                let schema = MetaSchema::new("boolean");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle arithmetic operations
            Expr::Plus(_, _, _) |
            Expr::Minus(_, _, _) |
            Expr::Multiply(_, _, _) |
            Expr::Divide(_, _, _) => {
                let schema = MetaSchema::new("number");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            },

            // Handle other expressions
            _ => {
                let schema = MetaSchema::new("object");
                Ok(MetaSchemaRef::Inline(Box::new(schema)))
            }
        }
    }
}

// Helper function to recursively fix additionalProperties in the schema
pub fn fix_additional_properties(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // First, recursively process all nested objects before handling additionalProperties
            // This ensures we process from the bottom up
            
            // Process properties if this is an object
            if let Some(serde_json::Value::Object(props)) = map.get_mut("properties") {
                for (_, prop_schema) in props.iter_mut() {
                    fix_additional_properties(prop_schema);
                }
            }
            
            // Process array items
            if let Some(items) = map.get_mut("items") {
                fix_additional_properties(items);
            }
            
            // Process oneOf, anyOf, allOf schemas
            for key in ["oneOf", "anyOf", "allOf"].iter() {
                if let Some(serde_json::Value::Array(variants)) = map.get_mut(*key) {
                    for variant in variants.iter_mut() {
                        fix_additional_properties(variant);
                    }
                }
            }
            
            // Process nested references and definitions
            if let Some(serde_json::Value::Object(defs)) = map.get_mut("definitions") {
                for (_, def_schema) in defs.iter_mut() {
                    fix_additional_properties(def_schema);
                }
            }
            
            // After processing nested elements, handle this object's additionalProperties
            
            // First check if this is an object type schema
            let is_object_type = map.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "object")
                .unwrap_or(false);
            
            // Remove any invalid additional properties from the schema object itself
            let valid_keys = vec![
                "type", "properties", "required", "additionalProperties", 
                "items", "oneOf", "anyOf", "allOf", "definitions",
                "title", "description", "format", "nullable",
                "minItems", "maxItems", "uniqueItems", "$ref"
            ];
            
            let keys_to_remove: Vec<String> = map.keys()
                .filter(|k| !valid_keys.contains(&k.as_str()))
                .cloned()
                .collect();
            
            for key in keys_to_remove {
                map.remove(&key);
            }
            
            if is_object_type {
                // For object types, ensure additionalProperties is set to false
                map.insert("additionalProperties".to_string(), serde_json::Value::Bool(false));
            }
            
            // Also handle objects that don't explicitly declare type: "object" but have properties
            if map.contains_key("properties") && !is_object_type {
                map.insert("type".to_string(), serde_json::Value::String("object".to_string()));
                map.insert("additionalProperties".to_string(), serde_json::Value::Bool(false));
            }
        }
        serde_json::Value::Array(arr) => {
            // Recursively process array elements
            for v in arr.iter_mut() {
                fix_additional_properties(v);
            }
        }
        _ => {}
    }
}

impl Default for RibConverter {
    fn default() -> Self {
        Self::new()
    }
}