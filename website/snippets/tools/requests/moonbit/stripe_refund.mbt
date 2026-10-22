pub async fn stripe_refund(
  revealed_key : String, charge : String, amount : Int,
) -> @http.Response {
  // Golem adds an idempotency key unless you set one:
  // Stripe honors that key, so a retry refunds once:
  @http.send(form_post( //@
    "https://api.stripe.com/v1/refunds",
    [("charge", charge), ("amount", amount.to_string())],
    headers=[("Authorization", "Bearer " + revealed_key)],
  )).unwrap()
}
