use protobuf_to_wit::convert_protobuf_to_wit;

const TODO_PROTO: &str = r#"syntax = "proto3";
package core.todo.v1;

message TodoAddRequest {
  string user_id = 1;
  string task = 2;
}

message TodoAddResponse { string message = 1; }

service TodoService {
  rpc TodoAdd(TodoAddRequest) returns (TodoAddResponse);
}
"#;

#[test]
fn renders_header_and_interface() {
    let out = convert_protobuf_to_wit(TODO_PROTO).expect("convert");
    let wit = out.wit_text;
    assert!(wit.starts_with("package core:v1@1.0.0;"));
    assert!(wit.contains("record todo-add-request {"));
    assert!(wit.contains("interface todo-service {"));
    assert!(wit.contains("todo-add: func(request: todo-add-request) -> result<todo-add-response, todo-error>;"));
} 