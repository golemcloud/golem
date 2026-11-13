// the agent receives an opaque handle, not the key:
final case class SupportConfig(stripeKey: Secret[String]) //@
    derives Schema

@agentDefinition()
trait SupportAgent extends BaseAgent
    with AgentConfig[SupportConfig]:
  class Id(val ticket: String)
  def issueRefund(charge: String, amount: Int): Future[Refund]

@agentImplementation()
final class SupportAgentImpl(
  ticket: String, config: Config[SupportConfig],
) extends SupportAgent:
  def issueRefund(charge: String, amount: Int): Future[Refund] =
    // refund needs a grant to reveal it; each reveal is recorded: //@
    // a binding can let a tool forward handles but reveal none: //@
    RefundToolClient().refund(
      charge, amount, config.value.stripeKey.handle(), //@
    ).map(_.orThrow)
