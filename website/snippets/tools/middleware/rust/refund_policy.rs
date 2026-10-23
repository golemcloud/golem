fn refuse(why: &str) -> ToolInvokeError<AppError> {
  ToolInvokeError::ConstraintViolation(why.into())
}
// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
#[tool_middleware(name = "refund-policy", //@
  constructor = RefundPolicy::new)]
impl RefundMiddleware for RefundPolicy { //@
  async fn refund(&self, underlying: &RefundUnderlying,
    charge: String, amount: u64, api_key: GuestSecretHandle,
    caller: Principal,
  ) -> Result<Receipt, ToolInvokeError<AppError>> {
    // reject calls by their arguments and enforce a rate limit:
    if amount > 100_000 { return Err(refuse("over the limit")); } //@
    check_rate_limit(&caller).await?; //@
    if amount > 10_000 {
      // the durable caller can wait for a person across restarts:
      let approval = create_promise(); //@
      ask_finance(&approval, &charge, amount).await?;
      let verdict = await_promise(&approval).await; //@
      if !approved(&verdict) { return Err(refuse("declined")); } //@
    }
    // forwarding cannot widen the agent's permissions:
    let receipt = underlying.refund(charge.clone(), amount, api_key)
      .await?;
    // record the refund and hide the customer's email:
    audit(&charge, amount, &receipt.id).await?; //@
    Ok(Receipt { customer_email: "[redacted]".into(), ..receipt }) //@
  }
}
