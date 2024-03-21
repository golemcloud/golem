use golem_openapi_client_generator::parse_openapi_specs;
use std::path::Path;

fn main() {
    golem_openapi_client_generator::gen(
        parse_openapi_specs(&[Path::new("../openapi/cloud-spec.yaml").to_path_buf()])
            .expect("Failed to parse OpenAPI spec."),
        Path::new("."),
        "golem-cloud-client",
        "0.0.0",
        false,
    )
    .expect("Failed to generate client code from OpenAPI spec.")
}
