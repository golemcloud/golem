use regex::Regex;

pub fn parse_proto_package(src: &str) -> Option<String> {
    let re = Regex::new(r"(?m)^\s*package\s+([A-Za-z0-9_\.]+)\s*;\s*$").ok()?;
    let caps = re.captures(src)?;
    Some(caps.get(1)?.as_str().to_string())
}

#[derive(Debug, Clone)]
pub struct MessageField {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone)]
pub struct OneofField {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone)]
pub struct OneofDef {
    pub name: String,
    pub options: Vec<OneofField>,
}

#[derive(Debug, Clone)]
pub struct MessageDef {
    pub name: String,
    pub fields: Vec<MessageField>,
    pub oneofs: Vec<OneofDef>,
}

#[derive(Debug, Clone)]
pub struct RpcDef {
    pub name: String,
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub name: String,
    pub rpcs: Vec<RpcDef>,
}

pub fn parse_messages(src: &str) -> Vec<MessageDef> {
    // Extremely naive regex-based parser for demo/golden tests
    let msg_re =
        Regex::new(r"(?s)message\s+([A-Za-z0-9_]+)\s*\{((?:[^{}]|\{[^{}]*\})*)\}").unwrap();
    let field_re =
        Regex::new(r"(?m)^\s*([A-Za-z0-9_\.]+)\s+([A-Za-z0-9_]+)\s*=\s*[0-9]+\s*;\s*$").unwrap();
    let oneof_re = Regex::new(r"(?s)oneof\s+([A-Za-z0-9_]+)\s*\{(.*?)\}").unwrap();
    let mut out = Vec::new();
    for caps in msg_re.captures_iter(src) {
        let name = caps.get(1).unwrap().as_str().to_string();
        let body = caps.get(2).unwrap().as_str();
        let mut fields = Vec::new();
        for f in field_re.captures_iter(body) {
            let ty = f.get(1).unwrap().as_str().to_string();
            let fname = f.get(2).unwrap().as_str().to_string();
            fields.push(MessageField { name: fname, ty });
        }
        let mut oneofs = Vec::new();
        for oc in oneof_re.captures_iter(body) {
            let oname = oc.get(1).unwrap().as_str().to_string();
            let obody = oc.get(2).unwrap().as_str();
            let mut options = Vec::new();
            for f in field_re.captures_iter(obody) {
                let ty = f.get(1).unwrap().as_str().to_string();
                let fname = f.get(2).unwrap().as_str().to_string();
                options.push(OneofField { name: fname, ty });
            }
            oneofs.push(OneofDef {
                name: oname,
                options,
            });
        }
        out.push(MessageDef {
            name,
            fields,
            oneofs,
        });
    }
    out
}

pub fn parse_service(src: &str) -> Option<ServiceDef> {
    let svc_re = Regex::new(r"(?s)service\s+([A-Za-z0-9_]+)\s*\{(.*?)\}").ok()?;
    let rpc_re = Regex::new(r"(?m)^\s*rpc\s+([A-Za-z0-9_]+)\s*\(\s*([A-Za-z0-9_\.]+)\s*\)\s*returns\s*\(\s*([A-Za-z0-9_\.]+)\s*\)\s*;\s*$").ok()?;
    let caps = svc_re.captures(src)?;
    let name = caps.get(1)?.as_str().to_string();
    let body = caps.get(2)?.as_str();
    let mut rpcs = Vec::new();
    for rc in rpc_re.captures_iter(body) {
        rpcs.push(RpcDef {
            name: rc.get(1).unwrap().as_str().to_string(),
            input: rc.get(2).unwrap().as_str().to_string(),
            output: rc.get(3).unwrap().as_str().to_string(),
        });
    }
    Some(ServiceDef { name, rpcs })
}

#[cfg(test)]
mod tests {
    use super::{parse_messages, parse_proto_package, parse_service};

    #[test]
    fn parses_package() {
        let src = r#"syntax = "proto3";
package core.todo.v1;
message X { string id = 1; }
"#;
        let pkg = parse_proto_package(src).unwrap();
        assert_eq!(pkg, "core.todo.v1");
    }

    #[test]
    fn parses_message_and_service() {
        let src = r#"syntax = "proto3";
package core.todo.v1;
message TodoAddRequest { string user_id = 1; string task = 2; }
message TodoAddResponse { string message = 1; }
service TodoService { rpc TodoAdd(TodoAddRequest) returns (TodoAddResponse); }
"#;
        let msgs = parse_messages(src);
        assert!(msgs.iter().any(|m| m.name == "TodoAddRequest"));
        let svc = parse_service(src).unwrap();
        assert_eq!(svc.name, "TodoService");
        assert_eq!(svc.rpcs.len(), 1);
    }

    #[test]
    fn parses_oneof() {
        let src = r#"syntax = "proto3";
package core.todo.v1;
message User {
  oneof id {
    string ssn = 1;
    int32 employee_id = 2;
  }
}
"#;
        let msgs = parse_messages(src);
        let user = msgs.into_iter().find(|m| m.name == "User").unwrap();
        assert_eq!(user.oneofs.len(), 1);
        assert_eq!(user.oneofs[0].name, "id");
        assert_eq!(user.oneofs[0].options.len(), 2);
    }
}
