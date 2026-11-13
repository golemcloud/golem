fn[E] refuse(val : String) -> @toolMiddleware.ToolInvokeError[E] {
  ConstraintViolation(val)
}
// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
#derive.tool_middleware("refund-policy", presented="Refund") //@
struct RefundPolicy {}

pub async fn RefundPolicy::refund(
  underlying : RefundUnderlying, charge : String, amount : Int,
  api_key : @secrets.SecretHandle[String],
  principal : @tool.Principal,
) -> Result[RefundResult,
  @toolMiddleware.ToolInvokeError[RefundError]] {
  // reject calls by their arguments and enforce a rate limit:
  if amount > 100_000 { return Err(refuse("over the limit")) } //@
  if check_rate_limit(principal) is Err(e) { return Err(e) } //@
  if amount > 10_000 {
    // the durable caller can wait for a person across restarts:
    let approval = @api.create_promise() //@
    ask_finance(approval, charge, amount)
    let verdict = @api.await_promise(approval) //@
    if !approved(verdict) { return Err(refuse("declined")) } //@
  }
  // forwarding cannot widen the agent's permissions:
  let forwarded = underlying.refund(charge, amount, api_key)
  guard forwarded is Ok(receipt) else { return forwarded }
  // record the refund and hide the customer's email:
  audit(charge, amount, receipt.id) //@
  Ok({ ..receipt, customer_email: "[redacted]" }) //@
}
