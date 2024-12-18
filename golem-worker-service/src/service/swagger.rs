pub struct SwaggerGenerator {
    pub swagger_ui_path: String
}
impl SwaggerGenerator {
    pub fn new(swagger_ui_path: String) -> Self {
        SwaggerGenerator {
            swagger_ui_path
        }
    }
}