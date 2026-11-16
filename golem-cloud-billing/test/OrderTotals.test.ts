import { describe, expect, it } from "vitest"
import { itemTotals } from "../src/OrderTotals.ts"

const item = (amount: number, proration = false) => ({ amount, proration })

describe("itemTotals", () => {
  it("gives each item its amount when the invoice has no discount and no tax on top", () => {
    expect(itemTotals({ items: [item(2000), item(345)], discountAmount: 0, totalAmount: 2345 })).toEqual([2000, 345])
  })

  it("shares the tax on top of the invoice over the items by their amounts", () => {
    expect(itemTotals({ items: [item(2000), item(345)], discountAmount: 0, totalAmount: 2626 })).toEqual([2240, 386])
  })

  it("takes the discount out of an invoice of paid overage alone", () => {
    // A coupon of 50% on $10.00 of paid overage, and 12% tax on the $5.00 after the discount.
    expect(itemTotals({ items: [item(1000)], discountAmount: 500, totalAmount: 560 })).toEqual([560])
  })

  it("shares the discount over the items that are not a proration", () => {
    // 50% off the $20.00 price and the $10.00 of paid overage. The proration credit of $5.00 keeps
    // its amount, and 10% tax applies to the $10.00 after the discount.
    expect(
      itemTotals({ items: [item(2000), item(-500, true), item(1000)], discountAmount: 1500, totalAmount: 1100 })
    ).toEqual([1100, -550, 550])
  })

  it("rounds the shares so that the items add up to the total of the invoice", () => {
    expect(itemTotals({ items: [item(100), item(100), item(100)], discountAmount: 0, totalAmount: 337 })).toEqual([
      113, 112, 112
    ])
  })

  it("gives nothing when the invoice charges nothing", () => {
    expect(itemTotals({ items: [item(2000), item(345)], discountAmount: 2345, totalAmount: 0 })).toEqual([0, 0])
    expect(itemTotals({ items: [], discountAmount: 0, totalAmount: 0 })).toEqual([])
  })
})
