use poem_openapi::Tags;

#[derive(Tags)]
pub enum ApiTags {
    ApiDefinition,
    Template,
    Worker,
    HealthCheck,
}
