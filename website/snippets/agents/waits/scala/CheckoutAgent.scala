// webhooks require this agent to have an HTTP mount:
@agentDefinition(mount = "/checkout/{orderId}")
trait CheckoutAgent extends BaseAgent:
  class Id(val orderId: String)
  def pay(amount: Int): Future[Unit]

@agentImplementation()
final class CheckoutAgentImpl(orderId: String)
    extends CheckoutAgent:
  def pay(amount: Int): Future[Unit] =
    val paid = HostApi.createWebhook()
    for
      _ <- createPaymentLink(orderId, amount, paid.url)
      // schedule a call on another agent for tomorrow:
      reminder <- MailerAgentClient.get(orderId)
        .remind.scheduleCancelableAt(inDays(1)) //@
      // it waits here for days if needed, using no CPU or memory:
      event <- paid.await().map(_.json[PaymentEvent]()) //@
      _ = reminder.cancel()
      _ <- if event.status != "succeeded" then Future.unit
           else
             // sleep for a week; its deadline survives restarts:
             sleep(7 * 24 * 60 * 60_000) //@
               .flatMap(_ => sendSurvey(orderId))
    yield ()
