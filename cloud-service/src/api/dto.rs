use poem_openapi::Object;

use crate::model::Account;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FindAccountsResponse {
    pub values: Vec<Account>,
}
