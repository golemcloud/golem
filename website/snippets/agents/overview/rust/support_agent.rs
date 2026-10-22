#[agent_definition(mount = "/tickets/{ticket}")]
pub trait SupportAgent {
  fn new(ticket: String) -> Self;
  async fn handle(&mut self, message: String, order_id: String)
    -> Result<String, AppError>;
}
struct SupportAgentImpl { history: Vec<Event> }
#[agent_implementation]
impl SupportAgent for SupportAgentImpl {
  fn new(_ticket: String) -> Self { Self { history: vec![] } }
  async fn handle(&mut self, message: String, order_id: String)
    -> Result<String, AppError> {
    // after a crash or restart, this history is restored:
    self.history.push(user(&message)); //@
    let reply: Reply = json(OpenAI::send(
      OpenAiConfig::from_env()?,
      self.history.clone(), json_model::<Reply>("gpt-4.1"),
    ).await)?;
    self.history.push(assistant(&reply.text));
    if !reply.refund_requested { return Ok(reply.text); }
    let approval = create_webhook()?;
    notify_reviewer(approval.url(), &order_id, reply.amount).await?;
    // it can wait here for days, then resume mid-function:
    let Approval { approved } = approval.await.json()?; //@
    // it refunds once if the payment API honors idempotency keys:
    if approved { refund_order(&order_id, reply.amount).await?; }
    Ok(reply.text)
  }
}
