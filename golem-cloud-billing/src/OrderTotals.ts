/**
 * What an order asks for each item, in cents: `amount`, the part of the order that the person
 * pays, shared over the items. Polar gives the amount of each item before the discount and the tax,
 * and computes the discount, the tax and the credit only for the whole order: the discount applies
 * to the items that are not a proration, and the tax to the subtotal after the discount. So the
 * discount of the order is shared over those items by their amounts, and `amount` over all items by
 * their amounts after the discount. The total of an order holds the tax when Polar adds it on top,
 * and the subtotal holds it when the tax is inclusive, so the result does not depend on the tax
 * behavior.
 */
export const itemTotals = (order: {
  readonly items: ReadonlyArray<{ readonly amount: number; readonly proration: boolean }>
  readonly discountAmount: number
  readonly amount: number
}): ReadonlyArray<number> => {
  const discounts = share(
    order.discountAmount,
    order.items.map((item) => (item.proration ? 0 : item.amount))
  )
  return share(
    order.amount,
    order.items.map((item, index) => item.amount - (discounts[index] ?? 0))
  )
}

/**
 * Splits `amount` by `weights` into whole cents that add up to `amount`: each share is rounded
 * down, and the cents that are left go to the largest remainders, the first item first on a tie.
 * Nothing to split, or no positive weight, gives 0 to each.
 */
const share = (amount: number, weights: ReadonlyArray<number>): ReadonlyArray<number> => {
  const sum = weights.reduce((total, weight) => total + weight, 0)
  if (amount <= 0 || sum <= 0) {
    return weights.map(() => 0)
  }
  const exact = weights.map((weight) => (amount * weight) / sum)
  const shares = exact.map((value) => Math.floor(value))
  let left = amount - shares.reduce((total, value) => total + value, 0)
  const byRemainder = exact
    .map((value, index) => ({ index, remainder: value - Math.floor(value) }))
    .sort((a, b) => b.remainder - a.remainder || a.index - b.index)
  for (const { index } of byRemainder) {
    if (left <= 0) break
    shares[index] = (shares[index] ?? 0) + 1
    left -= 1
  }
  return shares
}
