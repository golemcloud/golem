use openapi_to_wit::convert_openapi_to_wit;

const TODO_OPENAPI_SNIPPET: &str = r#"openapi: '3.0.3'
info:
  title: Todo REST API
  version: '1.0.0'
paths: {}
"#;

#[test]
fn generates_wit_package_and_version() {
    let out = convert_openapi_to_wit(TODO_OPENAPI_SNIPPET).expect("convert");
    assert_eq!(out.package, "api:todos");
    assert_eq!(out.version, "1.0.0");
} 