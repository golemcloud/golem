export const stripeRefund = (
  key: string, charge: string, amount: number,
) =>
  // Golem adds an idempotency key unless you set one:
  // a retry refunds once if Stripe honors that key:
  HttpClientRequest.post("https://api.stripe.com/v1/refunds").pipe( //@
    HttpClientRequest.bearerToken(key),
    HttpClientRequest.bodyUrlParams({ charge, amount }),
    HttpClient.execute,
  )
