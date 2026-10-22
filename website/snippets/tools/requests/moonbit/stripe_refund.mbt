pub async fn stripe_refund(
  key : String, charge : String, amount : Int,
) -> @http.Response {
  // Golem adds an idempotency key unless you set one:
  // a retry refunds once if Stripe honors that key:
  @http.send(form_post( //@
    "https://api.stripe.com/v1/refunds",
    [("charge", charge), ("amount", amount.to_string())],
    headers=[("Authorization", "Bearer " + key)],
  )).unwrap()
}
