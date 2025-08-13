use crate::naming::to_wit_ident;
use crate::parse::{MessageDef, RpcDef, ServiceDef};

pub struct WitPackage {
    pub name: String,
    pub version: String,
}

impl WitPackage {
    pub fn from_proto_package(proto_pkg: &str, version: &str) -> Self {
        let parts: Vec<&str> = proto_pkg.split('.').collect();
        let leaf = parts.last().copied().unwrap_or("api");
        let name = format!("core:{}", to_wit_ident(leaf));
        Self { name, version: version.to_string() }
    }

    pub fn header(&self) -> String {
        format!("package {}@{};\n\n", self.name, self.version)
    }
}

pub fn map_proto_scalar(t: &str) -> &'static str {
    match t {
        "string" => "string",
        "bool" => "bool",
        "int32" => "s32",
        "int64" => "s64",
        "uint32" => "u32",
        "uint64" => "u64",
        "float" => "float32",
        "double" => "float64",
        _ => "string",
    }
}

pub fn render_message_record(msg: &MessageDef) -> String {
    let name = to_wit_ident(&msg.name);
    let mut out = String::new();
    out.push_str(&format!("record {} {{\n", name));
    for f in &msg.fields {
        let fname = to_wit_ident(&f.name);
        let fty = map_proto_scalar(&f.ty);
        out.push_str(&format!("    {}: {},\n", fname, fty));
    }
    out.push_str("}\n\n");
    out
}

pub fn render_error_variant() -> String {
    let mut out = String::new();
    out.push_str("variant todo-error {\n");
    out.push_str("    not-found,\n");
    out.push_str("    unauthorized,\n");
    out.push_str("    invalid-input,\n");
    out.push_str("    internal-error,\n");
    out.push_str("}\n\n");
    out
}

pub fn render_service_interface(svc: &ServiceDef) -> String {
    let iname = to_wit_ident(&svc.name);
    let mut out = String::new();
    out.push_str(&format!("interface {} {{\n", iname));
    for rpc in &svc.rpcs {
        out.push_str(&render_rpc(rpc));
    }
    out.push_str("}\n\n");
    out
}

fn render_rpc(rpc: &RpcDef) -> String {
    let rname = to_wit_ident(&rpc.name);
    let input = to_wit_ident(&rpc.input);
    let output = to_wit_ident(&rpc.output);
    format!("    {}: func(request: {}) -> result<{}, todo-error>;\n", rname, input, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::MessageField;

    #[test]
    fn renders_message_and_service() {
        let msg = MessageDef { name: "TodoAddRequest".into(), fields: vec![MessageField { name: "user_id".into(), ty: "string".into() }] };
        let rec = render_message_record(&msg);
        assert!(rec.contains("record todo-add-request {"));
        assert!(rec.contains("user-id: string,"));

        let svc = ServiceDef { name: "TodoService".into(), rpcs: vec![RpcDef { name: "TodoAdd".into(), input: "TodoAddRequest".into(), output: "TodoAddResponse".into() }] };
        let iface = render_service_interface(&svc);
        assert!(iface.contains("interface todo-service {"));
        assert!(iface.contains("todo-add: func(request: todo-add-request) -> result<todo-add-response, todo-error>;"));
    }
} 