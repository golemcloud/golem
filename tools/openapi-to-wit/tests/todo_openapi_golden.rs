use openapi_to_wit::convert_openapi_to_wit;

const TODO_OPENAPI_SNIPPET: &str = r##"openapi: '3.0.3'
info:
  title: Todo REST API
  version: '1.0.0'
paths:
  /todos:
    post:
      operationId: CreateTodo
      requestBody:
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/TodoCreate"
      responses:
        "200":
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/Todo"
components:
  schemas:
    TodoCreate:
      type: object
      properties:
        title: { type: string }
    Todo:
      type: object
      properties:
        id: { type: string }
        title: { type: string }
"##;

#[test]
fn generates_wit_package_and_version_and_interface() {
    let out = convert_openapi_to_wit(TODO_OPENAPI_SNIPPET).expect("convert");
    assert_eq!(out.package, "api:todo-rest-api");
    assert_eq!(out.version, "1.0.0");
    let wit = out.wit_text;
    assert!(wit.contains("record todo-create {"));
    assert!(wit.contains("record todo {"));
    assert!(wit.contains("variant http-error"));
    assert!(wit.contains("interface todos {"));
    assert!(wit.contains("create-todo: func(request: todo-create) -> result<todo, http-error>;"));
} 