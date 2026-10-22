enum RefundError:
  @error(kind = "runtime", exitCode = 1) case Http

@toolDefinition(name = "refund", version = "1.0.0")
trait RefundTool:
  def refund(
    charge: String, amount: Int,
    // apiKey is an opaque handle; revealing it takes a grant:
    apiKey: GuestSecretHandle,
  ): Future[Either[RefundError, Refund]]

@toolImplementation()
final class RefundToolImpl extends RefundTool:
  // each call gets a fresh VM that can't read the agent's memory:
  def refund(charge: String, amount: Int, apiKey: GuestSecretHandle) //@
    : Future[Either[RefundError, Refund]] =
    val key = SecretApi.reveal[String](apiKey)
    // on replay, a completed request returns its recorded response:
    stripeRefund(key, charge, amount).flatMap: res => //@
      if !res.ok then Future.successful(Left(RefundError.Http))
      else res.text().toFuture.map(t => Right(Refund.parse(t)))
