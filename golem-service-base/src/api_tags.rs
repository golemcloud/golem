use poem_openapi::Tags;

#[derive(Tags)]
pub enum ApiTags {
    Template,
    Worker,
    HealthCheck,
}
