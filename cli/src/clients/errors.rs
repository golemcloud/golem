use golem_client::apis::account_api::{
    V2AccountsAccountIdDeleteError, V2AccountsAccountIdGetError, V2AccountsAccountIdPlanGetError,
    V2AccountsAccountIdPutError, V2AccountsPostError,
};
use golem_client::apis::grant_api::{
    V2AccountsAccountIdGrantsGetError, V2AccountsAccountIdGrantsRoleDeleteError,
    V2AccountsAccountIdGrantsRoleGetError, V2AccountsAccountIdGrantsRolePutError,
};
use golem_client::apis::login_api::{
    LoginOauth2DeviceCompletePostError, LoginOauth2DeviceStartPostError, V2LoginTokenGetError,
};
use golem_client::apis::project_api::{
    V2ProjectsDefaultGetError, V2ProjectsGetError, V2ProjectsPostError,
    V2ProjectsProjectIdDeleteError,
};
use golem_client::apis::project_grant_api::V2ProjectsProjectIdGrantsPostError;
use golem_client::apis::project_policy_api::{
    V2ProjectPoliciesPostError, V2ProjectPoliciesProjectPolicyIdGetError,
};
use golem_client::apis::token_api::{
    V2AccountsAccountIdTokensGetError, V2AccountsAccountIdTokensPostError,
    V2AccountsAccountIdTokensTokenIdDeleteError, V2AccountsAccountIdTokensTokenIdGetError,
};
use golem_client::apis::worker_api::{
    V2TemplatesTemplateIdWorkersPostError, V2TemplatesTemplateIdWorkersWorkerNameDeleteError,
    V2TemplatesTemplateIdWorkersWorkerNameGetError,
    V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError,
    V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError,
    V2TemplatesTemplateIdWorkersWorkerNameInvokePostError,
    V2TemplatesTemplateIdWorkersWorkerNameKeyPostError,
};

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl ResponseContentErrorMapper for V2AccountsAccountIdGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdPlanGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdPlanGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdPlanGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdPlanGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdPlanGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdPlanGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdPutError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdPutError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdPutError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdPutError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdPutError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdPutError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsPostError {
    fn map(self) -> String {
        match self {
            V2AccountsPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdDeleteError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdDeleteError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdDeleteError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdDeleteError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdGrantsGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdGrantsGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdGrantsGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdGrantsGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdGrantsGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdGrantsGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdGrantsRoleGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdGrantsRoleGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdGrantsRoleGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdGrantsRolePutError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdGrantsRolePutError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdGrantsRolePutError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdGrantsRolePutError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdGrantsRolePutError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdGrantsRolePutError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdGrantsRoleDeleteError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdGrantsRoleDeleteError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdGrantsRoleDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleDeleteError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdGrantsRoleDeleteError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2LoginTokenGetError {
    fn map(self) -> String {
        match self {
            V2LoginTokenGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2LoginTokenGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2LoginTokenGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2LoginTokenGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for LoginOauth2DeviceStartPostError {
    fn map(self) -> String {
        match self {
            LoginOauth2DeviceStartPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            LoginOauth2DeviceStartPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            LoginOauth2DeviceStartPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            LoginOauth2DeviceStartPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for LoginOauth2DeviceCompletePostError {
    fn map(self) -> String {
        match self {
            LoginOauth2DeviceCompletePostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            LoginOauth2DeviceCompletePostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            LoginOauth2DeviceCompletePostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            LoginOauth2DeviceCompletePostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectPoliciesPostError {
    fn map(self) -> String {
        match self {
            V2ProjectPoliciesPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectPoliciesPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectPoliciesPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectPoliciesPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectPoliciesPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectPoliciesPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectPoliciesProjectPolicyIdGetError {
    fn map(self) -> String {
        match self {
            V2ProjectPoliciesProjectPolicyIdGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectPoliciesProjectPolicyIdGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectPoliciesProjectPolicyIdGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectPoliciesProjectPolicyIdGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectPoliciesProjectPolicyIdGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectPoliciesProjectPolicyIdGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectsPostError {
    fn map(self) -> String {
        match self {
            V2ProjectsPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectsPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectsPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectsPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectsPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectsPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectsGetError {
    fn map(self) -> String {
        match self {
            V2ProjectsGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectsGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectsGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectsGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectsGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectsGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectsDefaultGetError {
    fn map(self) -> String {
        match self {
            V2ProjectsDefaultGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectsDefaultGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectsDefaultGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectsDefaultGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectsDefaultGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectsDefaultGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectsProjectIdDeleteError {
    fn map(self) -> String {
        match self {
            V2ProjectsProjectIdDeleteError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectsProjectIdDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectsProjectIdDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectsProjectIdDeleteError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectsProjectIdDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectsProjectIdDeleteError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2ProjectsProjectIdGrantsPostError {
    fn map(self) -> String {
        match self {
            V2ProjectsProjectIdGrantsPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2ProjectsProjectIdGrantsPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2ProjectsProjectIdGrantsPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2ProjectsProjectIdGrantsPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2ProjectsProjectIdGrantsPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2ProjectsProjectIdGrantsPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdTokensGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdTokensGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdTokensGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdTokensGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdTokensGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdTokensGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdTokensTokenIdGetError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdTokensTokenIdGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdTokensTokenIdGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdTokensPostError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdTokensPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdTokensPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdTokensPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdTokensPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdTokensPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2AccountsAccountIdTokensTokenIdDeleteError {
    fn map(self) -> String {
        match self {
            V2AccountsAccountIdTokensTokenIdDeleteError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2AccountsAccountIdTokensTokenIdDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdDeleteError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2AccountsAccountIdTokensTokenIdDeleteError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersPostError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameKeyPostError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameKeyPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokeAndAwaitPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameInvokePostError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInvokePostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameInterruptPostError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameDeleteError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameDeleteError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V2TemplatesTemplateIdWorkersWorkerNameGetError {
    fn map(self) -> String {
        match self {
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status404(error) => {
                format!("NotFound: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status409(error) => {
                format!("AlreadyExists: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V2TemplatesTemplateIdWorkersWorkerNameGetError::UnknownValue(json) => {
                format!("Unexpected error: {json:?}")
            }
        }
    }
}
