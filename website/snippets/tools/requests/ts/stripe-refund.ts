export async function stripeRefund(
  revealedKey: string, charge: string, amount: number,
): Promise<Response> {
  // Golem adds an idempotency key unless you set one:
  // Stripe honors that key, so a retry refunds once:
  return fetch('https://api.stripe.com/v1/refunds', { //@
    method: 'POST',
    headers: { Authorization: 'Bearer ' + revealedKey },
    body: new URLSearchParams({ charge, amount: String(amount) }),
  })
}
