use std::collections::HashMap;
use openapiv3::*;
use crate::api::types::*;

#[derive(Debug, thiserror::Error)]
pub enum OpenAPIError {
    #[error("Invalid OpenAPI specification: {0}")]
    InvalidSpec(String),
}

pub struct OpenAPIConverter;

impl OpenAPIConverter {
    pub fn new() -> Self {
        Self
    }

    pub fn convert(&self, api: &ApiDefinition) -> Result<OpenAPI, OpenAPIError> {
        let mut components = Components::default();
        
        // Convert security schemes
        if let Some(security) = &api.security {
            components.security_schemes = self.convert_security_schemes(security);
        }

        // Convert schemas
        let schemas = self.convert_schemas(&api.types);
        components.schemas = schemas;

        Ok(OpenAPI {
            openapi: "3.0.0".to_string(),
            info: Info {
                title: api.name.clone(),
                version: api.version.clone(),
                description: Some(api.description.clone()),
                ..Info::default()
            },
            paths: self.convert_paths(&api.routes)?,
            components: Some(components),
            security: self.convert_security_requirements(&api.security),
            ..OpenAPI::default()
        })
    }

    fn convert_security_schemes(&self, security: &SecuritySchemes) -> SecuritySchemes {
        let mut schemes = SecuritySchemes::default();
        
        for (name, scheme) in security.iter() {
            schemes.insert(name.clone(), self.convert_security_scheme(scheme));
        }
        
        schemes
    }

    fn convert_security_scheme(&self, scheme: &SecurityScheme) -> SecuritySchemeData {
        match scheme {
            SecurityScheme::ApiKey { header, .. } => SecuritySchemeData::ApiKey {
                name: header.clone(),
                location: ApiKeyLocation::Header,
                description: None,
            },
            SecurityScheme::OAuth2 { flows, .. } => SecuritySchemeData::OAuth2 {
                flows: self.convert_oauth_flows(flows),
            },
            // Add other security scheme types as needed
        }
    }

    fn convert_type(&self, t: &Type) -> Schema {
        match t {
            Type::String => Schema {
                schema_type: Some(SchemaType::String),
                ..Schema::default()
            },
            Type::Number => Schema {
                schema_type: Some(SchemaType::Number),
                ..Schema::default()
            },
            Type::Object { properties } => {
                let props: HashMap<_, _> = properties.iter()
                    .map(|(k, v)| (k.clone(), self.convert_type(v)))
                    .collect();
                    
                Schema {
                    schema_type: Some(SchemaType::Object),
                    properties: props,
                    ..Schema::default()
                }
            },
            // Add other type conversions
        }
    }
}

// Add caching support
pub struct CachedOpenAPIConverter {
    converter: OpenAPIConverter,
    cache: HashMap<String, OpenAPI>,
}

impl CachedOpenAPIConverter {
    pub fn new() -> Self {
        Self {
            converter: OpenAPIConverter::new(),
            cache: HashMap::new(),
        }
    }

    pub fn convert(&mut self, api: &ApiDefinition) -> Result<OpenAPI, OpenAPIError> {
        let cache_key = format!("{}:{}", api.id, api.version);
        
        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let spec = self.converter.convert(api)?;
        self.cache.insert(cache_key, spec.clone());
        Ok(spec)
    }
}

pub fn validate_openapi(spec: &OpenAPI) -> Result<(), OpenAPIError> {
    // ...existing code...
}
