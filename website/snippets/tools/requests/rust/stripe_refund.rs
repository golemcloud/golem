pub async fn stripe_refund(
  revealed_key: &str, charge: &str, amount: u64,
) -> Result<http::Response, AppError> {
  // Golem adds an idempotency key unless you set one:
  // Stripe honors that key, so a retry refunds once:
  let form = [("charge", charge), ("amount", &amount.to_string())];
  let req = Request::post("https://api.stripe.com/v1/refunds") //@
    .header(AUTHORIZATION, format!("Bearer {revealed_key}"))
    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
    .body(serde_urlencoded::to_string(form)?.into_bytes())?;
  Ok(http::send(req).await?) //@
}
