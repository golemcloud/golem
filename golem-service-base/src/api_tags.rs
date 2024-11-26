use poem_openapi::Tags;

#[derive(Tags)]
pub enum ApiTags {
    ApiDeployment,
    ApiDefinition,
    ApiSecurity,
    Component,
    Worker,
    HealthCheck,
    Plugin,
}
