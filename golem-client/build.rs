use std::path::Path;

use golem_openapi_client_generator::parse_openapi_specs;

fn main() {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[Path::new("../openapi/golem-service.yaml").to_path_buf()])
            .expect("Failed to parse OpenAPI spec."),
        Path::new("."),
        "golem-client",
        "0.0.0",
        false,
    )
        .expect("Failed to generate client code from OpenAPI spec.")
}
