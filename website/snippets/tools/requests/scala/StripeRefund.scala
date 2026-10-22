def stripeRefund(
  revealedKey: String, charge: String, amount: Int,
): Future[Response] =
  // Golem adds an idempotency key unless you set one:
  // Stripe honors that key, so a retry refunds once:
  fetch("https://api.stripe.com/v1/refunds", new RequestInit { //@
    method = HttpMethod.POST
    headers = js.Dictionary(
      "Authorization" -> ("Bearer " + revealedKey))
    body = URLSearchParams(js.Dictionary(
      "charge" -> charge, "amount" -> amount.toString))
  }).toFuture
