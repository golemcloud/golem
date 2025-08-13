use protobuf_to_wit::convert_protobuf_to_wit;

const PROTO: &str = r#"syntax = "proto3";
package core.todo.v1;

message TodoId { string id = 1; }

message UserIdOrSsn {
  oneof uid {
    string ssn = 1;
    string employee_id = 2;
  }
}

message GetRequest { string id = 1; }
message GetResponse { string title = 1; }

message SetOwnerRequest { string id = 1; UserIdOrSsn owner = 2; }
message SetOwnerResponse { string status = 1; }

service TodoService {
  rpc Get(GetRequest) returns (GetResponse);
  rpc SetOwner(SetOwnerRequest) returns (SetOwnerResponse);
}
"#;

#[test]
fn renders_two_rpcs_and_oneof_variant() {
    let out = convert_protobuf_to_wit(PROTO).expect("convert");
    let wit = out.wit_text;
    assert!(wit.contains("record get-request {"));
    assert!(wit.contains("record set-owner-request {"));
    assert!(wit.contains("variant user-id-or-ssn-uid {"));
    assert!(wit.contains("get: func(request: get-request) -> result<get-response, todo-error>;"));
    assert!(wit.contains(
        "set-owner: func(request: set-owner-request) -> result<set-owner-response, todo-error>;"
    ));
}
