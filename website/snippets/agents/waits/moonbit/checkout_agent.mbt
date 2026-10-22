#derive.agent
// webhooks require this agent to have an HTTP mount:
#derive.mount("/checkout/{order_id}")
struct CheckoutAgent {
  order_id : String
}

fn CheckoutAgent::new(order_id : String) -> CheckoutAgent {
  { order_id, }
}

pub async fn CheckoutAgent::pay(self : Self, amount : Int) -> Unit {
  let paid = @webhook.create()
  create_payment_link(self.order_id, amount, paid.url())
  // schedule a call on another agent for tomorrow:
  let mailer = MailerAgentClient::get(self.order_id)
  defer mailer.drop()
  let reminder = mailer.schedule_cancelable_remind(in_days(1)) //@
  // it waits here for days if needed, using no CPU or memory:
  let body = @json.parse(paid.wait().text()) //@
  let event : PaymentEvent = @json.from_json(body)
  reminder.cancel()
  if event.status != "succeeded" { return }
  // sleep for a week; its deadline survives restarts:
  @monotonicClock.wait_for(7UL * 24 * 60 * 60 * 1_000_000_000) //@
  send_survey(self.order_id)
}
