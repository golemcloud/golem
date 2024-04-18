use poem_openapi::Tags;

#[derive(Tags)]
pub enum ApiTags {
    ApiDeployment,
    ApiDefinition,
    Template,
    Worker,
    HealthCheck,
}
