use golem_rust::Schema;

#[derive(Schema)]
pub struct StringPathVarResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct MultiPathVarsResponse {
    pub joined: String,
}

#[derive(Schema)]
pub struct RemainingPathResponse {
    pub tail: String,
}


#[derive(Schema)]
pub struct PathAndQueryResponse {
    pub id: String,
    pub limit: u64,
}

#[derive(Schema)]
pub struct PathAndHeaderResponse {
    pub resource_id: String,
    pub request_id: String,
}

#[derive(Schema)]
pub struct JsonBodyResponse {
    pub ok: bool,
}

#[derive(Schema)]
pub struct JsonResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct OptionalResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct ResultOkResponse {
    pub value: String,
}

#[derive(Schema)]
pub struct ResultErrResponse {
    pub error: String,
}

#[derive(Schema)]
pub struct PreflightResponse {
    pub received: String,
}

#[derive(Schema)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Schema)]
pub struct PreflightRequest {
    pub name: String,
}
