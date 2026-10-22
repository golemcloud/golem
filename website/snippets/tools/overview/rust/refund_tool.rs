#[derive(ToolError)]
pub enum Failure {
  #[tool_error(kind = "runtime-error", exit_code = 1)] Http,
}

#[tool_definition(version = "1.0.0")]
pub trait Refund {
  async fn refund(&self, charge: String, amount: u64,
    // apiKey is an opaque handle; revealing it takes a grant:
    api_key: Secret) -> Result<Receipt, Failure>;
}
struct RefundImpl;
#[tool_implementation]
impl Refund for RefundImpl {
  // each call gets a fresh VM that can't read the agent's memory:
  async fn refund(&self, charge: String, amount: u64, //@
    api_key: Secret) -> Result<Receipt, Failure> {
    let key = api_key.with_handle(|k| reveal(k, &string_type()));
    let key = string_value(key);
    // on replay, a completed request returns its recorded response:
    let res = stripe_refund(&key, &charge, amount).await?; //@
    if !res.status().is_success() { return Err(Failure::Http); }
    Ok(http::response_json(res).await?)
  }
}
