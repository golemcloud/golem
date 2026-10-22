#derive.agent
#derive.mount("/tickets/{ticket}")
struct SupportAgent {
  ticket : String
  history : Array[@llm.Message]
}

fn SupportAgent::new(ticket : String) -> SupportAgent {
  { ticket, history: [] }
}

pub async fn SupportAgent::handle(
  self : Self, message : String, order_id : String,
) -> String {
  // after a crash or restart, this history is restored:
  self.history.push({ role: "user", content: message }) //@
  let reply : Reply = @llm.chat_object(
    self.history, model=@llm.openai("gpt-4.1"),
  )
  self.history.push({ role: "assistant", content: reply.text })
  if !reply.refund_requested { return reply.text }
  let approval = @webhook.create()
  notify_reviewer(approval.url(), order_id, reply.amount)
  // it can wait here for days, then resume mid-function:
  let body = @json.parse(approval.wait().text()) //@
  let { approved } : Approval = @json.from_json(body)
  // it refunds once if the payment API honors idempotency keys:
  if approved { refund_order(order_id, reply.amount) }
  reply.text
}
