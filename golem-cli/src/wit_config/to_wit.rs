use crate::wit_config::config::{WitConfig, Field, Function, Interface, Parameter, Record, ReturnTy, UseStatement, World};
use crate::wit_config::wit_types::WitType;
use convert_case::{Case, Casing};

static RESERVED_WORDS: &[&'static str] = &
    ["interface", "func", "record", "variant", "type", "unit", "string", "bool", "s32", "s64", "u32", "u64", "float32", "float64", "list", "option", "result", "import", "export", "as", "true", "false", "offset"];

/// Generate a WIT-compatible name by applying kebab-case, truncation, and reserved word handling.
fn generate_wit_name(name: &str) -> String {
    let kebab_case = name.to_case(Case::Kebab);

    // Prefix reserved words with %
    let mut final_name = if RESERVED_WORDS.iter().any(|v| *v == kebab_case.as_str()) {
        format!("%{}", kebab_case)
    } else {
        kebab_case
    };

    // Truncate to 64 characters
    if final_name.len() > 64 {
        final_name.truncate(64);
    }

    final_name
}

impl WitConfig {
    pub fn to_wit(&self) -> String {
        let package = format!("package {};\n", generate_wit_name(&self.package));
        let world = self.world.to_wit();
        let interfaces = self
            .interfaces
            .iter()
            .map(|interface| interface.to_wit())
            .collect::<Vec<String>>()
            .join("\n\n");

        format!("{}\n{}\n{}", package, interfaces, world)
    }
}

impl World {
    pub fn to_wit(&self) -> String {
        if self == &World::default() {
            return String::new();
        }

        let uses = self
            .uses
            .iter()
            .map(|use_statement| use_statement.to_wit())
            .collect::<Vec<String>>()
            .join("\n");

        let imports = self
            .imports
            .iter()
            .map(|interface| format!("import {};", generate_wit_name(&interface.name)))
            .collect::<Vec<String>>()
            .join("\n");

        let exports = self
            .exports
            .iter()
            .map(|interface| format!("export {};", generate_wit_name(&interface.name)))
            .collect::<Vec<String>>()
            .join("\n");

        format!(
            "world {} {{\n{}\n{}\n{}\n}}",
            generate_wit_name(&self.name),
            uses,
            imports,
            exports
        )
    }
}

impl Interface {
    pub fn to_wit(&self) -> String {
        let uses = self
            .uses
            .iter()
            .map(|use_statement| use_statement.to_wit())
            .collect::<Vec<String>>()
            .join("\n    ");

        let records = self
            .records
            .iter()
            .map(|record| record.to_wit())
            .collect::<Vec<String>>()
            .join("\n    ");

        let varients = self
            .varients
            .iter()
            .map(|(name, ty)| format!("{}", ty.to_wit(Some(generate_wit_name(name)))))
            .collect::<Vec<String>>()
            .join(", ");

        let functions = self
            .functions
            .iter()
            .map(|function| function.to_wit())
            .collect::<Vec<String>>()
            .join("\n    ");

        format!(
            "interface {} {{\n    {}\n    {}\n    {}\n    {}\n}}",
            generate_wit_name(&self.name),
            uses,
            records,
            varients,
            functions
        )
    }
}

impl Record {
    pub fn to_wit(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|field| field.to_wit())
            .collect::<Vec<String>>()
            .join(", ");

        format!("record {} {{ {} }}", generate_wit_name(&self.name), fields)
    }
}

impl Field {
    pub fn to_wit(&self) -> String {
        format!(
            "{}: {}",
            generate_wit_name(&self.name),
            self.field_type.to_wit(None)
        )
    }
}


impl Function {
    pub fn to_wit(&self) -> String {
        let params = self
            .parameters
            .iter()
            .map(|param| param.to_wit())
            .collect::<Vec<String>>()
            .join(", ");

        let return_type = if self.return_type.return_type.is_empty() {
            String::new()
        } else {
            format!(" -> {}", self.return_type.to_wit())
        };

        format!(
            "{}: func({}){};",
            generate_wit_name(&self.name),
            params,
            return_type
        )
    }
}

impl ReturnTy {
    pub fn to_wit(&self) -> String {
        if let Some(err) = self.error_type.as_ref() {
            format!("result<{}, {}>", self.return_type, err)
        } else {
            format!("option<{}>", self.return_type)
        }
    }
}

impl Parameter {
    pub fn to_wit(&self) -> String {
        format!(
            "{}: {}",
            generate_wit_name(&self.name),
            generate_wit_name(&self.parameter_type)
        )
    }
}

impl UseStatement {
    pub fn to_wit(&self) -> String {
        format!(
            "use {}.{{{}}};",
            generate_wit_name(&self.name),
            self.items
                .iter()
                .map(|item| generate_wit_name(item))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

impl WitType {
    pub fn to_wit(&self, name: Option<String>) -> String {
        match self {
            WitType::Bool => "bool".to_string(),
            WitType::U8 => "u8".to_string(),
            WitType::U16 => "u16".to_string(),
            WitType::U32 => "u32".to_string(),
            WitType::U64 => "u64".to_string(),
            WitType::S8 => "s8".to_string(),
            WitType::S16 => "s16".to_string(),
            WitType::S32 => "s32".to_string(),
            WitType::S64 => "s64".to_string(),
            WitType::Float32 => "float32".to_string(),
            WitType::Float64 => "float64".to_string(),
            WitType::Char => "char".to_string(),
            WitType::String => "string".to_string(),
            WitType::Option(inner) => format!("option<{}>", inner.to_wit(None)),
            WitType::Result(ok, err) => format!("result<{}, {}>", ok.to_wit(None), err.to_wit(None)),
            WitType::List(inner) => format!("list<{}>", inner.to_wit(None)),
            WitType::Tuple(elements) => format!(
                "tuple<{}>",
                elements.iter().map(|e| e.to_wit(None)).collect::<Vec<_>>().join(", ")
            ),
            WitType::Record(fields) => format!(
                "enum {} {{ {} }}",
                name.unwrap_or_default(),
                fields
                    .iter()
                    .map(|(name, ty)| format!("{}: {}", generate_wit_name(name), ty.to_wit(None)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WitType::Variant(variants) => format!(
                "variant {} {{ {} }}",
                name.unwrap_or_default(),
                variants
                    .iter()
                    .map(|(name, ty)| {
                        if let Some(ty) = ty {
                            format!("{}: {}", generate_wit_name(name), ty.to_wit(None))
                        } else {
                            generate_wit_name(name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WitType::Enum(variants) => format!(
                "enum {} {{ {} }}",
                name.unwrap_or_default(),
                variants
                    .iter()
                    .map(|variant| generate_wit_name(variant))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WitType::Flags(flags) => format!(
                "flags {{ {} }}",
                flags
                    .iter()
                    .map(|flag| generate_wit_name(flag))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WitType::Handle(name) => format!("handle<{}>", generate_wit_name(name)),
            WitType::TypeAlias(name, inner) => format!("type {} = {}", generate_wit_name(name), inner.to_wit(None)),
            WitType::FieldTy(name) => generate_wit_name(name)
        }
    }
}
