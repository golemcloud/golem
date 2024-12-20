use super::types::{OpenAPISpec, Schema, PathItem, ParameterLocation};
use std::collections::HashMap;

pub fn validate_openapi(spec: &OpenAPISpec) -> Result<(), String> {
    validate_paths(&spec.paths)?;
    validate_schemas(&spec.components.as_ref().unwrap().schemas)?;
    Ok(())
}

fn validate_paths(paths: &HashMap<String, PathItem>) -> Result<(), String> {
    for (path, item) in paths {
        if let Some(op) = &item.get {
            validate_operation(path, op)?;
        }
        if let Some(op) = &item.post {
             validate_operation(path, op)?;
        }
         if let Some(op) = &item.put {
              validate_operation(path, op)?;
        }
         if let Some(op) = &item.delete {
              validate_operation(path, op)?;
        }
    }
    Ok(())
}

fn validate_operation(path: &str, op: &super::types::Operation) -> Result<(), String> {
     if let Some(params) = &op.parameters {
            validate_parameters(path, params)?;
    }
    Ok(())
}


fn validate_parameters(path: &str, params: &Vec<super::types::Parameter>) -> Result<(), String> {
    for p in params.iter() {
        if p.r#in == ParameterLocation::Path {
             validate_path_parameter(path, p)?;
        }
    }
    Ok(())
}


fn validate_path_parameter(path: &str, p: &super::types::Parameter) -> Result<(), String> {
    let path_segments: Vec<&str> = path.split('/').collect();
      let matching_segments = path_segments
            .iter()
            .filter(|segment| segment.starts_with('{') && segment.ends_with('}'))
            .map(|segment| segment[1..segment.len() - 1].to_string()).collect::<Vec<_>>();
    
        if !matching_segments.iter().any(|s| s == &p.name) {
            return Err(format!(
                "Path parameter `{}` not found in path `{}`",
                p.name, path
            ));
        }
        Ok(())
}


fn validate_schemas(schemas: &Option<HashMap<String, Schema>>) -> Result<(), String> {
    if let Some(schemas) = schemas {
        for (key, schema) in schemas.iter() {
             validate_schema(key, schema)?;
        }
    }
      Ok(())
}

fn validate_schema(key: &String, schema: &Schema) -> Result<(), String> {
    match schema {
        Schema::Object { properties, .. } => {
            for (key, schema) in properties {
               validate_schema(key, schema)?;
            }
        }
         Schema::Array { items } => {
              validate_schema(&"array_item".to_string(), &items)?;
        }
          Schema::Ref { reference } => {
              validate_schema_ref(key, reference)?;
        }
        _ => {}
    }
      Ok(())
}

fn validate_schema_ref(_key: &String, reference: &String) -> Result<(), String> {
   if !reference.starts_with("#/components/schemas/") {
        return Err(format!(
            "Schema reference `{}` is invalid",
            reference
        ));
   }
      Ok(())
}