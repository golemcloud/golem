def stripeRefund(
  key: String, charge: String, amount: Int,
): Future[Response] =
  // Golem adds an idempotency key unless you set one:
  // a retry refunds once if Stripe honors that key:
  fetch("https://api.stripe.com/v1/refunds", new RequestInit { //@
    method = HttpMethod.POST
    headers = js.Dictionary("Authorization" -> ("Bearer " + key))
    body = URLSearchParams(js.Dictionary(
      "charge" -> charge, "amount" -> amount.toString))
  }).toFuture
