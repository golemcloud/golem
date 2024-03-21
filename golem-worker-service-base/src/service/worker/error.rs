use golem_common::model::{AccountId, TemplateId, WorkerId};
use golem_service_base::{
    model::{GolemError, VersionedTemplateId},
    service::auth::AuthError,
};

use crate::service::error::TemplateServiceBaseError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceBaseError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error(transparent)]
    Template(#[from] TemplateServiceBaseError),
    #[error("Type checker error: {0}")]
    TypeChecker(String),
    #[error("Template not found: {0}")]
    VersionedTemplateIdNotFound(VersionedTemplateId),
    #[error("Template not found: {0}")]
    TemplateNotFound(TemplateId),
    #[error("Account not found: {0}")]
    AccountIdNotFound(AccountId),
    // FIXME: Once worker is independent of account
    #[error("Worker not found: {0}")]
    WorkerNotFound(WorkerId),
    // TODO: FIX?
    #[error("Golem error")]
    Golem(GolemError),
}
