@agentDefinition(mount = "/tickets/{ticket}")
trait SupportAgent extends BaseAgent:
  class Id(val ticket: String)
  def handle(message: String, orderId: String): Future[String]

@agentImplementation()
final class SupportAgentImpl(ticket: String) extends SupportAgent:
  private val history = ArrayBuffer.empty[Message]

  def handle(message: String, orderId: String): Future[String] =
    // after a crash or restart, this history is restored:
    history += Message("user", message) //@
    openai.chat.completions.parse(ChatParams(
      model = "gpt-4.1", messages = history,
      responseFormat = responseFormat[Reply]("reply"),
    )).toFuture.flatMap: res =>
      val reply = Reply.parse(res.choices(0).message.parsed)
      history += Message("assistant", reply.text)
      if !reply.refundRequested then Future.successful(reply.text)
      else review(orderId, reply)

  private def review(orderId: String, reply: Reply) =
    val approval = HostApi.createWebhook()
    for
      _ <- notifyReviewer(approval.url, orderId, reply.amount)
      // it can wait here for days, then resume mid-function:
      approved <- approval.await().map(_.json[Approval]().approved) //@
      // it refunds once if the payment API honors idempotency keys:
      _ <- if approved then refundOrder(orderId, reply.amount)
           else Future.unit
    yield reply.text
