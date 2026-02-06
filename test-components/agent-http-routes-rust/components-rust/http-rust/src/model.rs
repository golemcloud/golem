pub struct StringPathVarResponse {
    pub value: String,
}

pub struct MultiPathVarsResponse {
    pub joined: String,
}

pub struct RemainingPathResponse {
    pub tail: String,
}

pub struct PathAndQueryResponse {
    pub id: String,
    pub limit: u64,
}

pub struct PathAndHeaderResponse {
    pub resource_id: String,
    pub request_id: String,
}

pub struct JsonBodyResponse {
    pub ok: bool,
}

pub struct JsonResponse {
    pub value: String,
}

pub struct OptionalResponse {
    pub value: String,
}

pub struct ResultOkResponse {
    pub value: String,
}

pub struct ResultErrResponse {
    pub error: String,
}

pub struct PreflightResponse {
    pub received: String,
}


pub struct OkResponse {
    pub ok: bool,
}

pub struct PreflightRequest {
    pub name: String,
}
