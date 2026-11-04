import type { Account } from "./Registry.ts"
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
    <link rel="stylesheet" href="/assets/app.css">
    <script src="/assets/app.js" defer></script>
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
      return "GitHub sign-in was cancelled."
    case "account_not_linked":
      return "Another GitHub account signed in with this email address before. Sign in with that GitHub account."
    case "unable_to_create_session":
      return "Signed in with GitHub, but Golem could not link the sign-in to your Golem account. Try again in a minute."
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
        <p class="lead">Sign in with the GitHub account that you use with the Golem CLI. Both sign-ins open the same Golem account.</p>
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
export type PlanState = "free" | "paid" | "paidCapped" | "other" | "unknown"

export interface SignedInPageOptions {
  readonly email: string
  readonly golemAccountId: string
  /** The account as the registry returns it; `undefined` when the registry call failed. */
  readonly account: Account | undefined
  readonly plan: PlanState
  /** The subscription of the paid product that changed last, if the account had one. */
  readonly subscription: Subscription | undefined
  /** Polar has redirected back after a payment, and the plan has not changed yet. */
  readonly checkoutPending: boolean
  readonly cliCommand: string
}

const longDate = new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "UTC" })

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
    ? html`<p class="lead">Your subscription ends on ${date}. The account moves to Free then.</p>`
    : html`<p class="lead">Your subscription renews on ${date}.</p>`
}

const planHero = (options: SignedInPageOptions) => {
  if (options.checkoutPending) {
    return html`<span class="eyebrow">Your plan</span>
        <h1>Activating Paid</h1>
        <p class="lead">Polar has the payment. Your plan changes to Paid within a minute; reload the page to see it.</p>`
  }
  switch (options.plan) {
    case "paid":
    case "paidCapped":
      return html`<span class="eyebrow">Your plan</span>
        <h1>You're on Paid</h1>
        ${renewal(options.subscription)}
        ${
          options.subscription?.status === "past_due"
            ? html`<p class="alert" role="alert">The last payment failed. Update the payment method in Manage billing to keep the Paid plan.</p>`
            : null
        }
        ${
          options.plan === "paidCapped"
            ? html`<p class="lead">Paid overage is off. Usage stops at the monthly amounts of Paid.</p>`
            : null
        }
        <div class="actions">
          ${portalForm("Manage billing")}
        </div>`
    case "free":
      return html`<span class="eyebrow">Your plan</span>
        <h1>You're on Free</h1>
        <p class="lead">Paid has larger monthly allowances. Polar handles the payment.</p>
        <div class="actions">
          ${checkoutForm}
          ${options.subscription === undefined ? null : portalForm("Billing history")}
        </div>`
    case "other":
      return html`<span class="eyebrow">Your plan</span>
        <h1>You're on a custom plan</h1>
        <p class="lead">Golem gave this account its plan. Contact Golem to change it.</p>`
    case "unknown":
      return html`<span class="eyebrow">Your plan</span>
        <h1>Plans and billing</h1>
        <p class="lead">Golem did not return the plan of the account. Reload the page to try again.</p>`
  }
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
      <div class="grid">
        <section class="card" aria-labelledby="account-heading">
          <h2 id="account-heading">Account</h2>
          <dl class="facts">
            ${
              options.account === undefined
                ? html`<div><dt>Details</dt><dd class="muted">Golem did not return the account details. Reload the page to try again.</dd></div>`
                : html`<div><dt>Name</dt><dd>${options.account.name}</dd></div>
            <div><dt>Email</dt><dd>${options.account.email}</dd></div>`
            }
            <div><dt>Account ID</dt><dd class="mono">${options.golemAccountId}</dd></div>
          </dl>
        </section>
        <section class="card" aria-labelledby="cli-heading">
          <h2 id="cli-heading">Use this account from the CLI</h2>
          <p>Run these commands to connect the Golem CLI to this account. The CLI opens GitHub in your browser to sign in.</p>
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
