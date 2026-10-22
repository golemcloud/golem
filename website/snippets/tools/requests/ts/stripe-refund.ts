export async function stripeRefund(
  key: string, charge: string, amount: number,
): Promise<Response> {
  // Golem adds an idempotency key unless you set one:
  // a retry refunds once if Stripe honors that key:
  return fetch('https://api.stripe.com/v1/refunds', { //@
    method: 'POST',
    headers: { Authorization: 'Bearer ' + key },
    body: new URLSearchParams({ charge, amount: String(amount) }),
  })
}
