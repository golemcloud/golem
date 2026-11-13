export const stripeRefund = (
  revealedKey: string, charge: string, amount: number,
) =>
  // Golem adds an idempotency key unless you set one:
  // Stripe honors that key, so a retry refunds once:
  HttpClientRequest.post("https://api.stripe.com/v1/refunds").pipe( //@
    HttpClientRequest.bearerToken(revealedKey),
    HttpClientRequest.bodyUrlParams({ charge, amount }),
    HttpClient.execute,
  )
