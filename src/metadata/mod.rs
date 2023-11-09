#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    pub name: Option<String>,
    pub producers: Option<Producers>,
    pub registry_metadata: Option<wasm_metadata::RegistryMetadata>,
}

/// https://github.com/WebAssembly/tool-conventions/blob/main/ProducersSection.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Producers {
    pub fields: Vec<ProducersField>,
}

impl From<wasm_metadata::Producers> for Producers {
    fn from(value: wasm_metadata::Producers) -> Self {
        let mut fields = Vec::new();
        for (name, field) in value.iter() {
            let name = name.clone();
            let mut values = Vec::new();
            for (name, version) in field.iter() {
                values.push(VersionedName {
                    name: name.clone(),
                    version: version.clone(),
                });
            }
            fields.push(ProducersField { name, values });
        }
        Producers { fields }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducersField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}
