use golem_rust::wasip3::clocks::monotonic_clock::wait_for as sleep;

// webhooks require this agent to have an HTTP mount:
#[agent_definition(mount = "/checkout/{order_id}")]
pub trait CheckoutAgent {
  fn new(order_id: String) -> Self;
  async fn pay(&mut self, amount: u64) -> Result<(), AppError>;
}
struct CheckoutAgentImpl { order_id: String }
#[agent_implementation]
impl CheckoutAgent for CheckoutAgentImpl {
  fn new(order_id: String) -> Self { Self { order_id } }
  async fn pay(&mut self, amount: u64) -> Result<(), AppError> {
    let paid = create_webhook()?;
    create_payment_link(&self.order_id, amount, paid.url()).await?;
    // schedule a call on another agent for tomorrow:
    let reminder = MailerAgentClient::get(self.order_id.clone())
      .schedule_cancelable_remind(in_days(1))?; //@
    // it waits here for days if needed, using no CPU or memory:
    let event: PaymentEvent = paid.await.json()?; //@
    reminder.cancel();
    if event.status != "succeeded" { return Ok(()); }
    // sleep for a week; its deadline survives restarts:
    sleep(7 * 24 * 60 * 60 * 1_000_000_000).await; //@
    send_survey(&self.order_id).await?;
    Ok(())
  }
}
