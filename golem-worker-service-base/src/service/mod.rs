use golem_common::model::TemplateId;
use golem_service_base::service::auth::Permission;

pub mod api_definition;
pub mod api_definition_validator;
pub mod http_request_definition_lookup;
pub mod template;
pub mod worker;

#[derive(Clone, Debug)]
pub struct TemplatePermission {
    pub template: TemplateId,
    pub permission: Permission,
}

impl TemplatePermission {
    pub fn new(template: TemplateId, permission: Permission) -> Self {
        Self {
            template,
            permission,
        }
    }
}
