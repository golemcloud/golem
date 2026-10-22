pub async fn stripe_refund(
  key: &str, charge: &str, amount: u64,
) -> Result<http::Response, AppError> {
  // Golem adds an idempotency key unless you set one:
  // a retry refunds once if Stripe honors that key:
  let form = [("charge", charge), ("amount", &amount.to_string())];
  let req = Request::post("https://api.stripe.com/v1/refunds") //@
    .header(AUTHORIZATION, format!("Bearer {key}"))
    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
    .body(serde_urlencoded::to_string(form)?.into_bytes())?;
  Ok(http::send(req).await?) //@
}
