def refuse(why: String) =
  Future.failed(Refused(ConstraintViolation(why)))
// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
@toolMiddleware(name = "refund-policy") //@
final class RefundPolicy extends RefundToolMiddleware:
  def refund(
    underlying: RefundToolUnderlying, charge: String, amount: Int,
    apiKey: GuestSecretHandle, caller: Principal,
  ): Future[Either[ToolInvokeError[RefundError], Refund]] =
    val done = for
      // reject calls by their arguments and enforce a rate limit:
      _ <- if amount > 100_000 then refuse("over the limit") //@
        else checkRateLimit(caller) //@
      _ <- if amount <= 10_000 then Future.unit else
        // the durable caller can wait for a person across restarts:
        val approval = HostApi.createPromise() //@
        for
          _ <- askFinance(approval, charge, amount)
          verdict <- HostApi.awaitPromise(approval) //@
          _ <- if approved(verdict) then Future.unit
            else refuse("declined") //@
        yield ()
      // forwarding cannot widen the agent's permissions:
      forwarded <- underlying.refund(charge, amount, apiKey)
        .toMiddlewareResult
      // record the refund and hide the customer's email:
      _ <- forwarded.fold(_ => Future.unit,
        r => audit(charge, amount, r.id)) //@
    yield forwarded.map(_.copy(customerEmail = "[redacted]")) //@
    done.recover { case Refused(error) => Left(error) }
