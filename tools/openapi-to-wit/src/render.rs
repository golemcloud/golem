use crate::naming::to_wit_ident;
use crate::parse::{ParsedField, ParsedOperation, ParsedRecord};

pub struct WitPackage {
    pub name: String,   // e.g., "api:todos"
    pub version: String, // e.g., "1.0.0"
}

impl WitPackage {
    pub fn render_header(&self) -> String {
        format!("package {}@{};\n\n", self.name, self.version)
    }
}

pub fn map_openapi_type(t: &str) -> &'static str {
    match t {
        "string" => "string",
        "boolean" => "bool",
        "integer" => "s32",
        "number" => "float64",
        _ => "string",
    }
}

pub fn render_record(rec: &ParsedRecord) -> String {
    let name = to_wit_ident(&rec.name);
    let mut out = String::new();
    out.push_str(&format!("record {} {{\n", name));
    for f in &rec.fields {
        out.push_str(&format!("    {}: {},\n", render_field_name(f), render_field_type(f)));
    }
    out.push_str("}\n\n");
    out
}

fn render_field_name(f: &ParsedField) -> String {
    to_wit_ident(&f.name)
}

fn render_field_type(f: &ParsedField) -> String {
    let base = map_openapi_type(&f.ty);
    if f.optional { format!("option<{}>", base) } else { base.to_string() }
}

pub fn render_error_variant() -> String {
    let mut out = String::new();
    out.push_str("variant http-error {\n");
    out.push_str("    bad-request,\n");
    out.push_str("    unauthorized,\n");
    out.push_str("    forbidden,\n");
    out.push_str("    not-found,\n");
    out.push_str("    internal-error,\n");
    out.push_str("}\n\n");
    out
}

pub fn render_interface(iface_name: &str, ops: &[ParsedOperation]) -> String {
    let mut out = String::new();
    out.push_str(&format!("interface {} {{\n", to_wit_ident(iface_name)));
    for op in ops {
        let oname = to_wit_ident(&op.operation_id);
        let req = op.request_record.as_ref().map(|s| to_wit_ident(s)).unwrap_or_else(|| "unit".to_string());
        let res = op.response_record.as_ref().map(|s| to_wit_ident(s)).unwrap_or_else(|| "unit".to_string());
        out.push_str(&format!("    {}: func(request: {}) -> result<{}, http-error>;\n", oname, req, res));
    }
    out.push_str("}\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::{render_error_variant, render_interface};
    use crate::parse::ParsedOperation;

    #[test]
    fn renders_interface_ops() {
        let ops = vec![ParsedOperation { operation_id: "CreateTodo".into(), request_record: Some("TodoCreate".into()), response_record: Some("Todo".into()) }];
        let iface = render_interface("todo-api", &ops);
        assert!(iface.contains("interface todo-api {"));
        assert!(iface.contains("create-todo: func(request: todo-create) -> result<todo, http-error>;"));
        let err = render_error_variant();
        assert!(err.contains("variant http-error"));
    }
} 