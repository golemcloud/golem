import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import type { OveragePrices } from "./BillingConfig.ts"
import type { Account, AccountLimits, MonthlyLimit } from "./Registry.ts"
import type { Subscription } from "./Subscriptions.ts"

/** HTML that `html` inserts as it is. */
export class SafeHtml {
  readonly value: string
  constructor(value: string) {
    this.value = value
  }
  toString(): string {
    return this.value
  }
}

const escapes: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;"
}

export const escapeHtml = (value: string): string => value.replace(/[&<>"']/g, (c) => escapes[c] ?? c)

type Interpolation = SafeHtml | string | number | ReadonlyArray<SafeHtml> | null | undefined

const render = (value: Interpolation): string => {
  if (value === null || value === undefined) return ""
  if (value instanceof SafeHtml) return value.value
  if (Array.isArray(value)) return value.map(render).join("")
  return escapeHtml(String(value))
}

/** A template tag that escapes every interpolated value that is not `SafeHtml`. */
export const html = (strings: TemplateStringsArray, ...values: ReadonlyArray<Interpolation>): SafeHtml =>
  new SafeHtml(strings.reduce((out, string, i) => out + string + (i < values.length ? render(values[i]) : ""), ""))

/**
 * An identifier that can wrap after each hyphen. Firefox does not break after a hyphen that a digit
 * follows, so a UUID such as `01a1c567-0cc6-7d91-…` otherwise wraps between two characters.
 */
export const breakAtHyphens = (value: string): SafeHtml => new SafeHtml(escapeHtml(value).replaceAll("-", "-<wbr>"))

/**
 * The URL of an asset with a hash of its content. The assets are cached for an hour, so a new
 * image would otherwise render its page with the stylesheet of the old one.
 */
const versionedAsset = (name: string): string => {
  const hash = createHash("sha256")
    .update(readFileSync(new URL(`./assets/${name}`, import.meta.url)))
    .digest("hex")
    .slice(0, 12)
  return `/assets/${name}?v=${hash}`
}

const stylesheetUrl = versionedAsset("app.css")
const scriptUrl = versionedAsset("app.js")

/** The default cloud URL of the Golem CLI; the CLI selects it with `--cloud`. */
const cliDefaultCloudUrl = "https://release.api.golem.cloud"

/**
 * The CLI commands that connect a local Golem CLI to the account of the signed-in user. The
 * CLI signs in with GitHub through the registry, which resolves the same GitHub identity to the
 * same Golem account.
 */
export const cliBootstrapCommand = (publicApiUrl: URL): string => {
  const url = publicApiUrl.href.replace(/\/$/, "")
  return url === cliDefaultCloudUrl
    ? "golem --cloud account get"
    : `golem profile new golem-cloud --url ${url} --set-active\ngolem account get`
}

const githubIcon = new SafeHtml(
  `<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.55 0-.27-.01-.99-.02-1.95-3.2.69-3.87-1.54-3.87-1.54-.52-1.33-1.28-1.68-1.28-1.68-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.71 1.26 3.37.96.1-.75.4-1.26.73-1.55-2.55-.29-5.24-1.28-5.24-5.67 0-1.25.45-2.27 1.18-3.07-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.18 1.17a11.05 11.05 0 0 1 5.78 0c2.2-1.48 3.17-1.17 3.17-1.17.63 1.58.23 2.75.11 3.04.74.8 1.18 1.82 1.18 3.07 0 4.4-2.69 5.38-5.25 5.66.41.36.78 1.06.78 2.14 0 1.55-.01 2.8-.01 3.18 0 .31.21.67.8.55A11.5 11.5 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z"/></svg>`
)

const layout = (options: { readonly title: string; readonly navUser?: SafeHtml; readonly body: SafeHtml }) =>
  html`<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex">
    <title>${options.title}</title>
    <link rel="icon" type="image/png" href="/assets/favicon.png">
    <link rel="stylesheet" href="${stylesheetUrl}">
    <script src="${scriptUrl}" defer></script>
  </head>
  <body>
    <header class="nav">
      <div class="container nav-inner">
        <a href="/" class="brand" aria-label="Golem Cloud billing home">
          <img src="/assets/golem-logo.png" alt="Golem" class="brand-logo" width="103" height="26">
          <span class="brand-product">Billing</span>
        </a>
        ${options.navUser}
      </div>
    </header>
    <main>
      <div class="container">
${options.body}
      </div>
    </main>
    <footer>
      <div class="container"><a href="https://golem.cloud">golem.cloud</a> · <a href="https://learn.golem.cloud">Documentation</a></div>
    </footer>
  </body>
</html>
`

/** Error codes that Better Auth appends to the error callback URL. */
const signInErrorMessage = (code: string): string => {
  switch (code) {
    case "access_denied":
      return "You cancelled the GitHub sign-in."
    case "account_not_linked":
      return "Another GitHub account signed in with this email address before. Sign in with that GitHub account."
    case "unable_to_create_session":
      return "GitHub accepted the sign-in, but Golem could not link it to your Golem account. Try again after one minute."
    default:
      return "Sign-in failed. Try again."
  }
}

export const signedOutPage = (options: { readonly error: string | undefined }) =>
  layout({
    title: "Golem Cloud billing",
    body: html`      <section class="hero">
        <span class="eyebrow">Golem Cloud</span>
        <h1>Plans and billing</h1>
        <p class="lead">Sign in with the GitHub account that you use with the Golem CLI. Then this page and the CLI use the same Golem account.</p>
        ${
          options.error === undefined
            ? null
            : html`<p class="alert" role="alert">${signInErrorMessage(options.error)} <code>${options.error}</code></p>`
        }
        <form method="post" action="/sign-in/github" class="actions">
          <button type="submit" class="btn btn-primary">${githubIcon} Sign in with GitHub</button>
        </form>
      </section>`
  })

/**
 * The plan of the account: one of the plans that a subscription switches between, another plan
 * that an administrator gave the account, or `unknown` when the registry call failed.
 */
export type PlanState = "free" | "paid" | "suspended" | "other" | "unknown"

/**
 * The result of the last post of the overage form, from the redirect after it: paid overage is
 * now on or off, or the plan of the account has no paid overage.
 */
export type OverageNotice = "on" | "off" | "not-eligible"

/**
 * How long a change of the monthly usage mode takes to reach the agents: the worker executors
 * read the limits of an account again every 5 minutes (`limit_refresh_interval`).
 */
const usageModeDelay = "5 minutes"

export interface SignedInPageOptions {
  readonly email: string
  readonly golemAccountId: string
  /** The account as the registry returns it; `undefined` when the registry call failed. */
  readonly account: Account | undefined
  readonly plan: PlanState
  /** The subscription of the paid product that changed last, if the account had one. */
  readonly subscription: Subscription | undefined
  /** When the past-due grace of the subscription ends, on the paid plan after a failed payment. */
  readonly pastDueGraceEnd: Date | undefined
  /** Polar has redirected back after a payment, and the plan has not changed yet. */
  readonly checkoutPending: boolean
  readonly cliCommand: string
  /** The monthly usage mode of a paid account; `undefined` on another plan or when the call failed. */
  readonly limits: AccountLimits | undefined
  readonly overagePrices: OveragePrices
  readonly overageNotice: OverageNotice | undefined
}

const longDate = new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "UTC" })

const longDateTime = new Intl.DateTimeFormat("en", { dateStyle: "long", timeStyle: "short", timeZone: "UTC" })

const checkoutForm = html`<form method="post" action="/billing/checkout">
            <button type="submit" class="btn btn-primary">Upgrade to Paid</button>
          </form>`

const portalForm = (label: string) => html`<form method="post" action="/billing/portal">
            <button type="submit" class="btn btn-secondary">${label}</button>
          </form>`

const renewal = (subscription: Subscription | undefined) => {
  if (subscription?.currentPeriodEnd === undefined) return null
  const date = longDate.format(subscription.currentPeriodEnd)
  return subscription.cancelAtPeriodEnd
    ? html`<p class="lead">Your subscription stops on ${date}. Then your plan changes to Free.</p>`
    : html`<p class="lead">Your subscription renews on ${date}.</p>`
}

const pastDueNotice = (end: Date) =>
  html`<p class="alert" role="alert">The last payment for Paid failed. Change the payment method in the Polar customer portal before ${longDateTime.format(end)} UTC. If Polar does not get the payment before then, your plan changes to Free. If the invoice also has paid overage, Golem stops the agents of this account until you pay it.</p>`

const planHero = (options: SignedInPageOptions) => {
  if (options.checkoutPending) {
    return html`<span class="eyebrow">Your plan</span>
        <h1>Polar has your payment</h1>
        <p class="lead">Your plan changes to Paid within one minute. Reload the page to see the change.</p>`
  }
  switch (options.plan) {
    case "paid":
      return html`<span class="eyebrow">Your plan</span>
        <h1>Paid</h1>
        ${options.pastDueGraceEnd === undefined ? renewal(options.subscription) : pastDueNotice(options.pastDueGraceEnd)}
        <div class="actions">
          ${portalForm("Manage billing")}
        </div>`
    case "suspended":
      return html`<span class="eyebrow">Your plan</span>
        <h1>Suspended</h1>
        <p class="alert" role="alert">Polar did not get the payment of an invoice for paid overage, so Golem stopped the agents of this account. Pay the invoice in the Polar customer portal. After the payment, the agents can run again.</p>
        <div class="actions">
          ${portalForm("Pay the invoice")}
        </div>`
    case "free":
      // A new checkout would start a second subscription; the payment of the first one is open.
      if (options.subscription?.status === "past_due") {
        return html`<span class="eyebrow">Your plan</span>
        <h1>Free</h1>
        <p class="alert" role="alert">The last payment for Paid failed, so your plan is Free. Change the payment method in the Polar customer portal. After the payment, your plan is Paid again.</p>
        <div class="actions">
          ${portalForm("Manage billing")}
        </div>`
      }
      return html`<span class="eyebrow">Your plan</span>
        <h1>Free</h1>
        <p class="lead">Paid has larger monthly allowances. You pay through Polar.</p>
        <div class="actions">
          ${checkoutForm}
          ${options.subscription === undefined ? null : portalForm("Billing history")}
        </div>`
    case "other":
      return html`<span class="eyebrow">Your plan</span>
        <h1>Custom</h1>
        <p class="lead">Golem set the plan of this account. To change the plan, ask Golem.</p>`
    case "unknown":
      return html`<span class="eyebrow">Your plan</span>
        <h1>Plans and billing</h1>
        <p class="lead">Golem did not send the plan of the account. Reload the page.</p>`
  }
}

const overageNoticeHtml = (notice: OverageNotice | undefined) => {
  switch (notice) {
    case undefined:
      return null
    case "on":
      return html`<p class="notice" role="status">Paid overage is on. Golem uses the new setting within ${usageModeDelay}.</p>`
    case "off":
      return html`<p class="notice" role="status">Paid overage is off. Golem uses the new setting within ${usageModeDelay}.</p>`
    case "not-eligible":
      return html`<p class="alert" role="alert">The plan of this account has no paid overage.</p>`
  }
}

/** How a meter of the current month shows its amounts: the unit, and a conversion from the meter. */
interface UsageMeter {
  readonly label: string
  readonly unit: string
  /** Converts an amount of the meter into the unit that the page shows. */
  readonly scale: number
  readonly format: Intl.NumberFormat
  /** The overage price of the meter, and the amount of the meter that the price is for. */
  readonly price: (prices: OveragePrices) => string
  readonly pricedAmount: number
}

const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 2 })
const oneDecimal = new Intl.NumberFormat("en", { maximumFractionDigits: 1 })
const twoDecimals = new Intl.NumberFormat("en", { maximumFractionDigits: 2 })
// An amount below one shows two significant digits, so that a small usage does not show as 0.
const small = new Intl.NumberFormat("en", { maximumSignificantDigits: 2 })
const usd = new Intl.NumberFormat("en", { style: "currency", currency: "USD" })

// Memory shows GB-hours, the unit of its overage price, not the GB-seconds of the meter.
const usageMeters = {
  computeGcu: {
    label: "Compute",
    unit: "GCU",
    scale: 1,
    format: compact,
    price: (prices) => prices.perMillionGcu,
    pricedAmount: 1_000_000
  },
  memoryGbSeconds: {
    label: "Memory",
    unit: "GB-hours",
    scale: 1 / 3600,
    format: oneDecimal,
    price: (prices) => prices.perGbHour,
    pricedAmount: 3600
  },
  durableStorageGbMonth: {
    label: "Durable storage",
    unit: "GB-month",
    scale: 1,
    format: twoDecimals,
    price: (prices) => prices.perDurableGbMonth,
    pricedAmount: 1
  },
  ephemeralStorageGbMonth: {
    label: "Ephemeral storage",
    unit: "GB-month",
    scale: 1,
    format: twoDecimals,
    price: (prices) => prices.perEphemeralGbMonth,
    pricedAmount: 1
  }
} as const satisfies Record<string, UsageMeter>

type MeterKey = keyof typeof usageMeters

const meterKeys = Object.keys(usageMeters) as Array<MeterKey>

/** The paid overage of a meter in USD, at the overage prices of the page. */
const overageCost = (meter: UsageMeter, limit: MonthlyLimit, prices: OveragePrices): number =>
  ((limit.allowOverageUsage ?? 0) / meter.pricedAmount) * Number(meter.price(prices))

const money = (value: number): string => (value > 0 && value < 0.01 ? "less than $0.01" : usd.format(value))

const usageItem = (meter: UsageMeter, limit: MonthlyLimit, prices: OveragePrices) => {
  const number = (value: number) => {
    const shown = value * meter.scale
    return shown > 0 && shown < 1 ? small.format(shown) : meter.format.format(shown)
  }
  const amount = (value: number) => `${number(value)} ${meter.unit}`
  if (limit.metering === "disabled") {
    return html`<div><dt>${meter.label}</dt><dd class="muted">Not metered</dd></div>`
  }
  if (limit.usage === undefined) {
    return html`<div><dt>${meter.label}</dt><dd class="muted">No usage this month</dd></div>`
  }
  if (limit.resolvedMonthlyAmount === undefined) {
    return html`<div><dt>${meter.label}</dt><dd>${amount(limit.usage)}, no monthly limit</dd></div>`
  }
  const allowance = limit.resolvedMonthlyAmount
  const used = Math.min(limit.usage, allowance)
  const exact = allowance > 0 ? (used / allowance) * 100 : 100
  const percent = used > 0 && exact < 1 ? "<1%" : `${Math.round(exact)}%`
  const overage = limit.allowOverageUsage ?? 0
  return html`<div>
            <dt>${meter.label}</dt>
            <dd>${number(used)} of ${amount(allowance)} included · ${percent}
              <progress class="${used >= allowance ? "full" : ""}" max="${Math.max(allowance, 1)}" value="${used}" aria-label="${meter.label} used">${percent}</progress>
            </dd>
            ${
              overage > 0
                ? html`<dd class="overage-amount">+ ${amount(overage)} of paid overage · ${money(overageCost(meter, limit, prices))}</dd>`
                : null
            }
          </div>`
}

/**
 * When Polar bills the paid overage. The product has no meter cycle of its own, so Polar bills
 * the usage of a period on the order at the end of the period: the renewal, or the last order of a
 * subscription that ends.
 */
const overageBilling = (subscription: Subscription | undefined): string => {
  if (subscription?.currentPeriodEnd === undefined) {
    return "Polar bills it on the next invoice."
  }
  const date = longDate.format(subscription.currentPeriodEnd)
  return subscription.cancelAtPeriodEnd
    ? `Polar bills it on the last invoice, when the subscription stops on ${date}.`
    : `Polar bills it on the invoice of ${date}, with the next month of the plan.`
}

/**
 * The usage push sends the overage to Polar on a schedule, and Polar prices the events some time
 * after it receives them, so the customer portal of Polar shows less than this page for a while.
 */
const polarDelay = "Polar shows this amount after a delay."

const usageSection = (limits: AccountLimits, prices: OveragePrices, subscription: Subscription | undefined) => {
  const monthly = limits.monthly
  if (monthly === undefined) {
    return html`<p class="muted">Golem did not send the usage of this month. Reload the page.</p>`
  }
  const total = meterKeys.reduce((sum, key) => sum + overageCost(usageMeters[key], monthly[key], prices), 0)
  return html`<p class="muted">This month (UTC), against the monthly allowance of the plan.</p>
        ${
          total > 0
            ? html`<p class="notice">Paid overage so far this month: about ${money(total)} plus tax, at the prices below. ${overageBilling(subscription)} ${polarDelay}</p>`
            : null
        }
        <dl class="facts usage">
          ${meterKeys.map((key) => usageItem(usageMeters[key], monthly[key], prices))}
        </dl>`
}

/**
 * The usage of the month against the allowance of a paid account, and its paid-overage switch with
 * the overage prices. Without the card, only the notice of the last post of the overage form shows.
 */
const usageCard = (options: SignedInPageOptions) => {
  if (options.plan !== "paid" || options.checkoutPending || options.limits?.overageAllowedByPlan !== true) {
    return overageNoticeHtml(options.overageNotice)
  }
  const on = options.limits.monthlyUsageMode === "allowOverage"
  const prices = options.overagePrices
  return html`<section class="card usage-card" aria-labelledby="usage-heading">
        <h2 id="usage-heading">Monthly usage</h2>
        ${usageSection(options.limits, prices, options.subscription)}
        <h3>Paid overage</h3>
        ${overageNoticeHtml(options.overageNotice)}
        <p>${
          on
            ? "On. When the account uses all of its monthly allowance, its agents continue to run. Polar bills the usage above the allowance at these prices."
            : "Off. When the account uses all of its monthly allowance, its agents stop until the next month."
        }</p>
        <dl class="facts prices">
          <div><dt>Compute</dt><dd>$${prices.perMillionGcu} per million GCU</dd></div>
          <div><dt>Memory</dt><dd>$${prices.perGbHour} per GB-hour</dd></div>
          <div><dt>Durable storage</dt><dd>$${prices.perDurableGbMonth} per GB-month</dd></div>
          <div><dt>Ephemeral storage</dt><dd>$${prices.perEphemeralGbMonth} per GB-month</dd></div>
        </dl>
        <p class="muted">Prices in USD, plus tax. Polar bills the overage of a month on the next invoice.</p>
        <form method="post" action="/billing/overage" class="actions">
          <input type="hidden" name="mode" value="${on ? "hardLimit" : "allowOverage"}">
          <button type="submit" class="btn ${on ? "btn-secondary" : "btn-primary"}">${on ? "Turn off paid overage" : "Turn on paid overage"}</button>
        </form>
        <p class="muted">Golem uses a new setting within ${usageModeDelay}.</p>
      </section>`
}

export const signedInPage = (options: SignedInPageOptions) =>
  layout({
    title: "Your plan · Golem Cloud",
    navUser: html`<div class="nav-user">
          <span class="nav-email">${options.email}</span>
          <form method="post" action="/sign-out"><button type="submit" class="btn btn-secondary btn-small">Sign out</button></form>
        </div>`,
    body: html`      <section class="hero">
        ${planHero(options)}
      </section>
      ${usageCard(options)}
      <div class="grid">
        <section class="card" aria-labelledby="account-heading">
          <h2 id="account-heading">Account</h2>
          <dl class="facts">
            ${
              options.account === undefined
                ? html`<div><dt>Details</dt><dd class="muted">Golem did not send the account details. Reload the page.</dd></div>`
                : html`<div><dt>Name</dt><dd>${options.account.name}</dd></div>
            <div><dt>Email</dt><dd>${options.account.email}</dd></div>`
            }
            <div><dt>Account ID</dt><dd class="mono">${breakAtHyphens(options.golemAccountId)}</dd></div>
          </dl>
        </section>
        <section class="card" aria-labelledby="cli-heading">
          <h2 id="cli-heading">Use this account from the CLI</h2>
          <p>Run these commands to connect the Golem CLI to this account. The CLI opens GitHub in your browser for the sign-in.</p>
          <div class="code">
            <div class="code-header">
              <span>Shell</span>
              <button type="button" class="btn btn-secondary btn-small" data-copy="cli-command" hidden>Copy</button>
            </div>
            <pre><code id="cli-command">${options.cliCommand}</code></pre>
          </div>
        </section>
      </div>`
  })
