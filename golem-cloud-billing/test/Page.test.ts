import { describe, expect, it } from "vitest"
import { breakAtHyphens, canCheckOut, canSettle, cliBootstrapCommand, html, type PlanState, SafeHtml } from "../src/Page.ts"
import type { Subscription } from "../src/Subscriptions.ts"

describe("canSettle", () => {
  const offer = { orders: [{ orderId: "order_1", amount: 345 }], amount: 345 }

  it.each<[PlanState, boolean]>([
    ["suspended", true],
    ["free", false],
    ["paid", false],
    ["default", false],
    ["other", false],
    ["unknown", false]
  ])("on the plan %s with a settlement gives %s", (plan, expected) => {
    expect(canSettle(plan, offer)).toBe(expected)
  })

  it("refuses without a settlement", () => {
    expect(canSettle("suspended", undefined)).toBe(false)
  })
})

describe("canCheckOut", () => {
  const subscription = (status: string): Subscription => ({
    subscriptionId: "sub_1",
    accountId: "01a1c567-0cc6-7d91-943e-998dafbebc78",
    productId: "115e800a-2b62-42e0-97bf-9e8a5d6022b3",
    status,
    currentPeriodEnd: undefined,
    cancelAtPeriodEnd: false,
    modifiedAt: new Date("2026-11-04T10:00:00Z"),
    meteredPriceIds: [],
    pastDueAt: undefined,
    endedAt: undefined
  })

  it.each<[PlanState, string, boolean]>([
    ["free", "none", true],
    ["free", "canceled", true],
    ["free", "past_due", false],
    ["paid", "active", false],
    ["paid", "past_due", false],
    ["suspended", "canceled", false],
    ["default", "none", true],
    ["default", "past_due", false],
    ["other", "none", false],
    ["unknown", "none", false]
  ])("on the %s plan with the subscription status %s: %s", (plan, status, expected) => {
    expect(canCheckOut(plan, status === "none" ? undefined : subscription(status))).toBe(expected)
  })
})

describe("html", () => {
  it("escapes interpolated strings", () => {
    expect(html`<p>${`<script>alert("x")</script>`}</p>`.value).toBe(
      "<p>&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;</p>"
    )
  })

  it("inserts SafeHtml as it is", () => {
    expect(html`<p>${new SafeHtml("<b>bold</b>")}</p>`.value).toBe("<p><b>bold</b></p>")
  })
})

describe("breakAtHyphens", () => {
  it("lets a UUID wrap only after its hyphens", () => {
    expect(breakAtHyphens("01a1c567-0cc6-7d91-943e-998dafbebc78").value).toBe(
      "01a1c567-<wbr>0cc6-<wbr>7d91-<wbr>943e-<wbr>998dafbebc78"
    )
  })

  it("escapes the value", () => {
    expect(breakAtHyphens("<a>-b").value).toBe("&lt;a&gt;-<wbr>b")
  })
})

describe("cliBootstrapCommand", () => {
  it("uses the built-in cloud profile for the default cloud URL", () => {
    expect(cliBootstrapCommand(new URL("https://release.api.golem.cloud"))).toBe("golem --cloud account get")
  })

  it("creates a profile for another Golem API URL", () => {
    expect(cliBootstrapCommand(new URL("https://release.dev-api.golem.cloud/"))).toBe(
      "golem profile new golem-cloud --url https://release.dev-api.golem.cloud --set-active\ngolem account get"
    )
  })
})
