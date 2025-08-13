use openapi_to_wit::convert_openapi_to_wit;

const TODO_OPENAPI_DOC: &str = r#"openapi: '3.0.3'
info:
  title: Todo REST API
  version: '1.0.0'
components:
  schemas:
    Todo:
      type: object
      required: [id, title]
      properties:
        id: { type: string }
        title: { type: string }
        completed: { type: boolean }
"#;

#[test]
fn renders_todo_record() {
    let out = convert_openapi_to_wit(TODO_OPENAPI_DOC).expect("convert");
    let wit = out.wit_text;
    assert!(wit.contains("package api:todo-rest-api@1.0.0;"));
    assert!(wit.contains("record todo {"));
    assert!(wit.contains("id: string,"));
    assert!(wit.contains("title: string,"));
    // completed is optional in our test document
    assert!(wit.contains("completed: option<bool>,"));
} 