# golem-cloud-billing

The billing app of the hosted Golem Cloud. Users sign in with GitHub, subscribe to the paid plan
through Polar.sh, and manage the subscription in the customer portal of Polar. Self-hosted Golem
does not use it.

The app calls the HTTP API of the registry service, and reads two metering tables and a few
account columns of the registry (see [Database](#database)). It uses nothing else in Golem, and
the registry knows nothing about the app. In the hosted deployment both use the same PostgreSQL
database: the registry keeps its tables in `public`, and the app keeps its tables in its own
schema.

## Stack

- [Effect 4](https://effect.website) for the HTTP server, configuration, services and the
  registry client.
- [Better Auth](https://better-auth.com) with the GitHub provider, mounted on the Effect router
  as described in its [Effect integration](https://better-auth.com/docs/integrations/effect).
- The [Polar SDK](https://www.npmjs.com/package/@polar-sh/sdk) at API version `2026-10` for
  checkouts, customer sessions and the webhook signature. The app does not use the Polar plugin of
  Better Auth: the plugin links a Polar customer to the Better Auth user ID, and the app links it
  to the Golem account ID, which the usage push also sends.
- Server-rendered HTML with no client framework. `src/assets/app.js` only copies the CLI command.

## Identity bridge

GitHub sign-in ends in the `session.create.before` hook of Better Auth (`src/Auth.ts`). Before
Better Auth stores the session, the hook:

1. posts the GitHub access token of the user to `POST /v1/login/oauth2` of the registry, which
   returns the Golem account and a Golem API token,
2. writes the account ID to `user.golem_account_id`,
3. stores the Golem API token on the session, encrypted with `BETTER_AUTH_SECRET`.

The registry keeps one Golem account per GitHub identity, so the CLI and the billing app reach the
same account, whichever signs in first. When a step fails, Better Auth creates no session and the
landing page shows the error.

Account linking is off: a GitHub identity signs in only to the user that it created, also when
another identity has its verified email. The hook refuses a user with more than one GitHub account,
and a Golem account other than the one in `user.golem_account_id`. After a delete, the registry
gives the GitHub identity a new Golem account at the next login, and the hook still refuses the
user of the deleted account. The cleanup (see [Deleted accounts](#deleted-accounts)) deletes the
Polar customer of the deleted account and then the user, and the next sign-in creates a new user
for the new account. So the checkout of the new account starts only after the old Polar customer
is gone.

## Database

The connections of the app set `search_path` to its schema (`billing` by default). The app keeps
its tables there: the tables of Better Auth (`user`, `session`, `account` and `verification`),
`polar_subscriptions`, `polar_orders`, `polar_checkouts`, `overage_settlements`,
`polar_webhook_deliveries`, `usage_events`, `deleted_account_cleanups`, `deletion_holds`,
`usage_settlements` and `usage_settlement_history`.

`node main.mjs migrate` creates and changes these tables. It is the only DDL of the app. It runs
once and exits, with a nonzero exit code when it fails, and needs only the `GOLEM__DB__*`
variables. It runs under a PostgreSQL advisory lock, so two runs migrate one after the other, and
a run on a migrated schema changes nothing. It does not create the schema: an administrator
creates the schema for the owner role first, and `migrate` fails without it. The web app,
`push-usage` and `cleanup-deleted-accounts` run no DDL. At start, each checks that the tables
exist, and fails with a message that names `migrate` when one is missing.

Better Auth reads the columns of every table in the database during `migrate`. When a registry
migration changes a table at the same moment, the read fails; `migrate` tries it again up to five
times.

In the hosted deployment, `ci-scripts/setup-cloud-billing.sh` of golem-cloud creates one role for
`migrate` and one for each workload. Each workload role has USAGE on the schemas `billing` and
`public`, and only the privileges that the workload needs. `test/registry.ts` gives the tests the
same grants.

- `golem_cloud_billing` owns the schema `billing` and every table in it. Only `migrate` uses it. It
  has no privilege on the registry tables.
- `golem_cloud_billing_web`, for the web app: SELECT, INSERT and UPDATE on `user`, `account`,
  `polar_subscriptions`, `polar_orders` and `polar_checkouts`; SELECT, INSERT, UPDATE and DELETE
  on `session` and `deletion_holds`; SELECT, INSERT and DELETE on `verification`; SELECT and
  INSERT on `overage_settlements` and `polar_webhook_deliveries`; SELECT on `usage_events` and
  `usage_settlements`; SELECT (`account_id`, `deleted_at`) on `public.accounts`.
- `golem_cloud_billing_push`, for `push-usage`: SELECT, INSERT and UPDATE on `usage_events`;
  SELECT on `polar_subscriptions`, `public.account_monthly_policy_revisions` and
  `public.account_monthly_policy_attribution`.
- `golem_cloud_billing_cleanup`, for `cleanup-deleted-accounts`: SELECT (`golem_account_id`) and
  DELETE on `user`; SELECT, INSERT and UPDATE (`polar_customer_deleted_at`) on
  `deleted_account_cleanups`; SELECT and DELETE on `deletion_holds`; SELECT on
  `polar_subscriptions`, `polar_orders`, `polar_checkouts`, `overage_settlements`, `usage_events`
  and `usage_settlements`; SELECT (`account_id`, `deleted_at`) on `public.accounts`. The ON DELETE
  CASCADE of `session` and `account` runs as the owner of those tables, so a delete of a user also
  deletes its sessions and GitHub accounts.

No workload role can create a table, or write `usage_settlements` or `usage_settlement_history`.
An operator records a usage settlement as the admin user (see [Deletion hold](#deletion-hold)).

The registry tables hold API token secrets and agent secrets, so the workloads read only these
registry data:

- two metering tables, for the usage push (see [Billable overage](#billable-overage)),
- `accounts (account_id, deleted_at)`, for the web app and the cleanup: the registry answers 404
  both for a deleted account and for an account that the billing account cannot see, and only
  this column tells them apart.

The other columns of `accounts`, for example the email, stay out of reach. Everything
else comes from the registry HTTP API. As the signed-in user, the app calls
`POST /v1/login/oauth2` at sign-in and `GET /v1/accounts/{account_id}` for the account page. As
the billing account, the checkout route, the webhooks and the sweep call `GET /v1/me/token`,
`GET /v1/accounts/{account_id}`, `PUT /v1/accounts/{account_id}/plan`, and `PUT` and `DELETE`
`/v1/accounts/{account_id}/deletion-holds/{holder_account_id}` (see
[Subscriptions](#subscriptions)).

## Subscriptions

A subscription of the Polar product `POLAR_PRODUCT_ID` and its orders move the Golem account
between three plans. The Polar customer has the Golem account ID as its external ID. The page and
the deletion hold show the paid plan as Pro.

The registry gives a new account its default plan (`initial_plans.default`). With
`GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID`, the page offers Pro to an account on that plan too, and a
subscription moves the account from it only to the paid plan. A trial, or a subscription that is
never active, leaves the account on the default plan. After a subscription the account follows
the table below, so its plan is free after the end and never goes back to the default plan.
Without the variable, the app treats the default plan like an internal plan that an
administrator gave the account.

| Plan | Variable | When |
| --- | --- | --- |
| suspended | `GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID` | paid overage is not paid: the account owes an unpaid order (see [Unpaid orders](#unpaid-orders)), its subscription is not in the past-due grace, and a payment of the subscription failed or the order is older than one hour |
| paid | `GOLEM_CLOUD_BILLING_PAID_PLAN_ID` | an `active` subscription, or a `past_due` subscription in the past-due grace, that does not end within 15 minutes |
| free | `GOLEM_CLOUD_BILLING_FREE_PLAN_ID` | otherwise: no subscription, a declined payment of the price alone after the past-due grace, a cancellation, a trial, a pause |

The paid plan is prepaid with optional postpay. Its price buys the monthly amounts in advance.
When a payment fails, the account keeps the paid plan for the past-due grace. When the payment of
the price is still open after the past-due grace, the account is on the free plan until the
payment, as without a subscription. Paid overage is billed after the period, for resources that
the account has used: when that payment is still open after the past-due grace, the account is
suspended until the order is paid or settled, also with a new subscription. The suspended plan has no monthly amounts and `overage_eligible: false`, so the
agents of the account stop. A renewal order that has the next price and the overage of the last
period counts as overage.

Only the paid plan allows overage. On a plan without overage the registry sets the account to
`hardLimit`. Only the owner can turn paid overage on again
(`PUT /v1/accounts/{account_id}/monthly-usage-mode`), also after the account is back on the paid
plan.

- **Checkout.** `POST /billing/checkout` creates a checkout session for the signed-in user and
  redirects to it: the paid product, the Golem account ID as `external_customer_id`, the email and
  the name of the user, and the IP address of the browser for the local currency. After the
  payment Polar redirects to `/?checkout_id=...`, and the page shows that the plan is activating
  until the webhook changes it. The page offers the checkout, and the route accepts it, only for
  an account on the free plan or on the default plan whose latest subscription is not `past_due`
  (`canCheckOut` in `src/Page.ts`). The route redirects any other request to `/`, also when the
  registry or the database fails. Under the lock of the account, the route records the checkout
  in `polar_checkouts` and places the [deletion hold](#deletion-hold) of the checkout before it
  redirects: the URL of the checkout holds the client secret that pays it, so the person can pay
  only a checkout that holds the account. When the database or the registry fails, the route
  shows the error page and never sends the URL. First the route reads from Polar each unfinished
  checkout of the account that is `open` or `confirmed`. While Polar has the payment of a checkout
  (`confirmed` or `succeeded`), each click redirects to `/?checkout_id=...` and starts no new
  checkout, also when the person paid in another tab. Else a click goes back to an open checkout
  that takes payments for more than an hour (`CHECKOUT_REUSE_MS`), so the hold does not grow with
  each click. The route never sends a checkout in a currency that the app has no prices for, also
  not an open one that a click reuses (see [Currencies](#currencies)).
- **Customer portal.** `POST /billing/portal` creates a customer session for the Golem account and
  redirects to the customer portal of Polar: payment method, invoices, cancellation.
- **Settlement.** `POST /billing/settle` creates a checkout session of the settlement product for
  the paid overage of the orders that Polar no longer collects, and redirects to it (see
  [Unpaid orders](#unpaid-orders)). After the payment Polar redirects to
  `/?settlement_orders=...`, with the IDs of the orders that the settlement pays. While one of them
  is unpaid, the page says that Golem records the payment. After the webhook, the page shows the
  plan and any remaining debt.
- **Webhooks.** Polar delivers `subscription.*`, `order.created`, `order.updated` and `order.paid`
  events to `POST /api/auth/polar/webhooks`. A request without the Standard Webhooks headers gets
  403 before the app reads its body, and a body above 1 MiB gets 413. The handler verifies the
  Standard Webhooks signature with `POLAR_WEBHOOK_SECRET` (403 when it does not match), skips a
  `webhook-id` that it handled before, and stores the subscription in `polar_subscriptions` or the
  order in `polar_orders`. A subscription links the checkout that started it (`checkout_id`) in
  `polar_checkouts`. A paid order of the settlement product records a settlement instead. A state
  never replaces a state with a later `modified_at`, so a late delivery cannot undo a newer one.
  The app keeps `modified_at` as Polar sends it, with microseconds, and only PostgreSQL compares
  it: a JavaScript `Date` would make two changes within one millisecond equal. Then it sets the
  [deletion hold](#deletion-hold) and the plan from the table above. A failed payment
  (`subscription.past_due`) also logs `payment_failed`. An account on the default plan moves only to
  the paid plan. An account on any other plan, for example an internal plan that an administrator
  gave it, keeps it, and gets the deletion hold all the same. An event of an account that the
  registry deleted is stored and answers 200, without a change of the plan or the hold, so that
  deleted accounts do not make Polar disable the endpoint.
- **Metered prices.** An order item names its price. The subscription lists its own prices, in its
  currency, and its product lists the prices of every currency. `polar_subscriptions` keeps the IDs
  of the metered prices of both lists, and the handler counts the items of these prices as
  overage.
- **Failed payments.** Polar sets `past_due_at` on the subscription when a payment fails, clears
  it when a payment succeeds, and keeps it when it revokes the subscription after the failed
  retries. When the last order of a subscription that has ended fails, Polar does not retry it
  and does not mark the subscription: only the age of the order shows the failure. Polar charges
  an order when it creates it, so a renewal stays on the paid plan while Polar charges it.
- **Past-due grace.** For `GOLEM_CLOUD_BILLING_PAST_DUE_GRACE` (7 days by default) after
  `past_due_at`, a `past_due` subscription keeps the account on the paid plan, as after a paid
  renewal: the monthly amounts and paid overage work as before. Polar retries the payment in this
  time, four times over 21 days, and at once when the customer changes the payment method. The
  page shows the failed payment and the time by which to pay. A subscription that Polar revokes is
  `canceled`, and that ends the past-due grace. The Polar setting "Grace period for benefit
  revocation" does not change the plan, because the app does not use Polar benefits.
- **Sweep.** Every minute, each replica of the web app sets the plan and the deletion hold of the
  accounts whose plan or hold changes with the time alone: a subscription that is canceled at the
  period end, a `pending` order that is older than one hour, a `past_due` subscription whose
  past-due grace ends, usage events that the usage push recorded, sent or held in the last 24
  hours for an account without a subscription that continues, and usage settlements of the last 24
  hours. Before that, it reads from Polar each unfinished checkout that is `open` or `confirmed`, 5
  minutes after its expiry (`CHECKOUT_READ_DELAY_MS`) and then every 5 minutes
  (`CHECKOUT_READ_INTERVAL_MS`), and records its status (see [Deletion hold](#deletion-hold)).
- **Order and retries.** An advisory lock per account serializes the webhooks, the checkout route,
  the sweep and the cleanup of one account, also across replicas. In one process, the callers of
  one account wait in a queue, and hold no database connection while they wait. The lock holds one
  connection of the pool, and the queries inside the lock run on it, so the holder never waits for
  a second connection. A pool of one connection works. When the registry or the database fails,
  the handler answers 500 and Polar delivers the webhook again. A 404 of the registry for an
  account that is not deleted also answers 500: the permission share of the billing account is
  missing. A 409 of the registry (the account changed between the read and the write) makes the
  handler read the account again.

The plan of a customer is not up to its owner: the registry refuses `set-plan` on the own account.
The app changes plans and deletion holds with the token of a separate billing account
(`GOLEM_CLOUD_BILLING_REGISTRY_TOKEN`). An administrator gives that account the three permissions
once, with a permission share from the administrator account
(`POST /v1/accounts/<administrator account ID>/permission-shares` with an administrator token):

```json
{
  "targetAccountEmail": "<email of the billing account>",
  "name": "cloud-billing-set-plan",
  "data": {
    "lowerPositive": [
      "account(*) @ <email of the billing account> : view :",
      "account(*) @ <email of the billing account> : set-plan :",
      "account(*) @ <email of the billing account> : hold-deletion :"
    ],
    "lowerNegative": [],
    "upperPositive": [],
    "upperNegative": []
  }
}
```

`set-plan` and `hold-deletion` need `view`, because the registry reads the account first. A holder
places and removes only its own deletion hold, so the owner of an account cannot remove the hold
of the billing account. The token of the billing account can read accounts, change plans and set
its deletion holds, and nothing else.

### Currencies

The paid plan has prices in USD, EUR and GBP (`src/Currency.ts`). The checkout sends the IP
address of the browser, and Polar selects the currency from the country of that address. When the
product has no price in the currency of that country, Polar uses USD, the default currency of the
Polar organization. A subscription keeps the currency of its checkout, and its orders have it too.
The app stores the currency of each subscription, order and settlement in
`polar_subscriptions.currency`, `polar_orders.currency` and `overage_settlements.currency`, as Polar
names it: `usd`, `eur` or `gbp`. An amount is in the minor unit of its currency: cents or pence.

Polar adds the tax on top of a USD price (`tax_behavior` `exclusive`), and takes it out of a EUR or
GBP price (`inclusive`), so a customer in the EU or the UK sees what they pay. The prices of the
Polar product must have this tax behavior: the page says "plus tax" next to a USD amount and
"including tax" next to a EUR or GBP amount.

The page shows the overage prices and the amounts of an account in the currency of its
subscription, with the prices of that currency in the configuration (see
[Configuration](#configuration)). When the page cannot read the subscription, it shows the usage
without prices or costs. Before a subscription, the page cannot know the currency: Polar selects it
only when it creates the checkout, and has no API that selects a currency without a checkout. So
next to the checkout the page shows the monthly price of the paid plan in each currency, and says
that the checkout of Polar shows the price before the payment. On the free and the default plan,
the page shows the usage without overage prices.

The checkout route reads the currency of the checkout that Polar returns. For a currency that the
app has no prices for, it logs the error `unsupported_currency` and shows the error page, and it
never sends the URL of the checkout. Polar selects the currency at the creation of a checkout, and
the billing address that the customer enters on the checkout page does not change it. So a later
click that reuses that open checkout gets the error page too, until the checkout takes payments for
less than an hour more and the route creates a new one. The app also stores no subscription or
order in such a currency: the webhook logs the error `unsupported_currency` and answers 200,
because a delivery again changes nothing. So the Polar product must have prices in USD, EUR and GBP
only.

### Settlement

Polar bills a usage event only if the customer has a billable subscription (`active`, `trialing`
or `past_due`) when Polar processes the event. Polar accepts a later event and does not bill it.
Polar also does not bill usage during a trial, and invoices the usage of a `past_due`
subscription only after the payment. The plans above keep the paid overage inside the time that
Polar bills:

- **Cancellation.** The account moves to the free plan 15 minutes before the scheduled end. The
  executors report usage every minute and the usage push runs every 10 minutes, so the push sends
  the last paid overage before the end.
- **Renewal.** Overage that reaches Polar after the period ends goes on the next invoice, at the
  same prices.
- **Failed payment.** In the past-due grace, the account uses paid overage as before, and Polar
  invoices it after the payment. If the payment does not come, Polar does not get paid for the
  overage of the past-due grace; the length of the past-due grace limits that amount. After the
  past-due grace, an account that did not pay for its paid overage uses nothing more.
- **Trial.** A trial gives the free plan. The paid product has no trial.
- **Product change.** The app has one paid product, so a subscription does not change its
  product. Polar invoices the usage of the old product at the change; a second product needs a
  push before that change.

The usage push does not send an event that Polar would not bill (see
[Billable overage](#billable-overage)).

### Unpaid orders

An order is unpaid while it is `pending`, or `void` with no settlement in `overage_settlements`.
The account owes the paid overage of an unpaid order: the amount of its items of metered prices.
Its price alone does not count.

Polar collects a `pending` order of a billable subscription: it retries the payment, and a new
default card in the customer portal pays it at once. Polar collects no other unpaid order:

- It does not retry the order of a subscription that has ended, such as the last order after a
  cancellation. The customer portal has no pay button for it, and the merchant API cannot charge
  it.
- It voids the pending orders of a subscription that it revokes, also after the failed retries of
  a payment. A void collects nothing, so the app does not let a void remove a debt.

The person pays the orders that Polar no longer collects with a settlement. On the suspended plan
the page offers "Settle" with the amount, in place of the customer portal, and `POST /billing/settle`
accepts it only then (`canSettle` in `src/Page.ts`, `settlementFor` in `src/Settlement.ts`). The
route reads the orders, their currency and the amount from the database, and opens a checkout of
the private one-time product `POLAR_SETTLEMENT_PRODUCT_ID` with an ad-hoc fixed price of that amount
in that currency, discount codes off, and the order IDs in the metadata `golem_settled_order_ids`.
The amount is what the invoices charged for the paid overage, after the discount and with the tax,
and the price includes tax (`tax_behavior` `inclusive`), so the person pays what the invoices asked.
Polar gives the discount and the tax only for a whole order, so the webhook shares them over the items
(`src/OrderTotals.ts`): the discount over the items that are not a proration, by their amounts,
and the total of the order over all items, by their amounts after the discount. The total holds
the tax when Polar adds it on top, and the subtotal holds it when the tax is inclusive, so no tax
setting of the organization or of a price changes the amount. Credit from the wallet of the
customer, for example after a downgrade, pays part of an order. A pending order keeps that credit,
so it shares its due amount. A void gives the credit back to the wallet, so a void order shares its
total. Polar also takes credit that the customer gained after the order against a void, and the
order does not show it; such a person pays that part twice, and an operator refunds it.

A checkout has one currency, so one settlement pays orders in one currency: the currency of the
oldest order that fits. It pays the oldest orders in that currency that fit: at most 13, the most
IDs that fit in a metadata value, and at most 999,999.99 in the currency, the largest price of
Polar. The orders in another currency wait for a later settlement. The checkout names the currency
(`currency`), so Polar charges in it whatever the address of the browser. Polar refuses a list of
prices without a price in the default currency of the organization, also for ad-hoc prices, and
each currency must have the same types of price. So a settlement in EUR or GBP also gets a fixed
USD price of 0. The customer can select that price through the public checkout update of Polar,
and the checkout currency then follows the price. Only Polar can refuse that change. At 0, the
change charges nothing: Polar makes a free order, and the webhook settles nothing.

A paid order of the settlement product records a settlement for each order in its metadata that
the account still owes. When the account owes one of those orders in another currency than the
settlement order, the app records nothing and logs `A settlement order is in another currency than
the orders that it settles`; the account stays suspended. The app reads the currency of each order
from its database, not from the checkout. When the account owes no more paid
overage, it moves to the plan of its subscriptions: the free plan, or the paid plan when the person subscribed again. A new
subscription does not end the suspension before that, and the page names the plan that follows.
A settlement order settles nothing when its price, the subtotal after the discount, is less than
the amount of those orders, and logs an error. An order that the account does not owe logs a warning; an operator refunds that part in the Polar dashboard. Polar sends a
paid order in several events, and the later ones change nothing. If Polar charges an original
order after its settlement, the app logs `An order was paid after its settlement`, and an operator
refunds the settlement order. The app has no `refunds:write`.

Polar refuses a fixed price below $0.50, €0.50 or £0.40. When the paid overage of an order that
Polar no longer collects is below $0.50, €0.50 or £0.50 in its currency (`SETTLEMENT_MINIMUM` in
`src/Settlement.ts`), the app writes the order off for good: it does not suspend or hold the
account, and other unpaid orders do not change this.

Only Polar support can void an original order or charge it again. Until then, the customer portal
lists it as pending, also after its settlement.

### Deletion hold

Golem does not charge early, and a cancellation runs to the end of the period: Polar bills paid
overage on the order at the end of the period. So the registry must not delete an account while
the account has a billable subscription or owes paid overage. A checkout can start a subscription
for 24 hours, so the registry must not delete the account of an unfinished checkout either. The
app places a deletion hold on the account through the registry (`Settlement.holdFor`) while any of
these is true, and removes it when none is:

| Reason | When |
| --- | --- |
| `subscription` | a billable subscription (`active`, `trialing` or `past_due`) is not canceled at the period end |
| `canceledSubscription` | a billable subscription is canceled at the period end and runs to the end of the period |
| `unbilledUsage` | a billable subscription, and a usage event that no order bills yet |
| `unsettledUsage` | no billable subscription, and a usage event that no order bills yet |
| `unpaidOverage` | the account owes an unpaid order (see [Unpaid orders](#unpaid-orders)) |
| `openCheckout` | a checkout of the paid product is `open`, and no subscription links it |
| `paidCheckout` | Polar took the payment of a checkout (`confirmed` or `succeeded`), and no subscription links it |

A checkout is unfinished from its creation until a subscription webhook links it
(`checkout_id`), or until Polar reports it `expired` or `failed`. Polar takes the payment of a
checkout for 24 hours, and has no API that closes a checkout earlier or makes it shorter. A
failed payment makes a `confirmed` checkout `open` again. A job of Polar moves the `open`
checkouts after their expiry to `expired`, every 15 minutes. Before the expiry, only the checkout
route reads a checkout, at a click of the account; else the row says `open`, and a payment brings
the subscription webhook. The sweep reads each unfinished checkout from Polar 5 minutes after its
expiry, and then every 5 minutes until Polar reports a final status: `succeeded`, `expired` or
`failed`. Only `expired` or `failed` removes the hold. Polar answers the read from a replica that
can lag, so an `open` checkout after its expiry can be one that the person paid just before it.
No status follows a final status, so a replica that lags never reports one too early, and the app
keeps a final status that it recorded. A checkout that Polar reports `confirmed` or `succeeded`
keeps the account held until its subscription comes, with no time limit. The checkouts of the
settlement product hold
nothing: the account owes paid overage while the page offers them, and a payment of a settlement
for orders that the account no longer owes logs a warning for an operator.

A usage event that no order bills yet is not held, and is not sent yet, or was sent after the
newest order of the account while the latest subscription of the account can still create an
order. Polar bills an event only after the push sends it, on the next order. The latest
subscription is a billable subscription, or else the subscription that ended last (`ended_at` of
Polar). An ended subscription creates no more orders when:

- Polar ended it before its period end: a revoke in the Polar dashboard, or the revoke of Polar
  after the failed retries. Polar keeps `current_period_end`, so `ended_at` is before it. Polar
  bills paid overage only on the order at the period end, and the product has no meter cycle, so
  Polar never bills the events that it received after the newest order. This overage is lost
  revenue, and it does not hold the account.
- Polar created its last order: an order of the subscription, created at or after its period end.
  No order bills anything after it, so it settles every sent event, also one whose `sent_at` the
  app recorded after that order.

An unpaid order with paid overage holds the account as `unpaidOverage`, also when it is the only
reason, because the customer can still pay it: in the customer portal while Polar collects it, and
with a settlement after that.

The hold stays for the whole life of a billable subscription, also after a cancellation. Paid
overage needs the paid plan, and the paid plan needs a billable subscription, so the account has a
hold for as long as it can run up paid overage. The owner turns paid overage on and off in the
registry, and no webhook tells the app, so a hold that followed the switch would come too late.
The account moves to the free plan `SETTLEMENT_MARGIN_MS` before a canceled subscription ends, so
the usage push records the last paid overage before Polar bills it.

A reconcile places the hold before it changes the plan, and removes the hold after it changes the
plan, so the account is never on the paid plan without a hold. The table `deletion_holds` records
the holds that the app placed. The sweep reconciles every account with a billable subscription and
no row there: a subscription from before the app placed holds, or one whose last placement failed.
It also reconciles the accounts without a billable subscription whose usage events changed, the
accounts with a new usage settlement, because no webhook comes for either, and the accounts with an
unfinished checkout and no row, whose placement in the checkout route failed. And it reconciles each
account whose row is older than an hour (`HOLD_RECHECK_MS`). A placement refreshes the row, and a
removal that failed keeps it, so the sweep tries the removal again with no time limit.

An `unsettledUsage` hold clears by itself within minutes: the push sends or holds an unsent event
at its next run, and Polar creates the last order soon after the period end. When it stays, for
example because the last order never comes, an operator checks the account in Polar and records a
usage settlement with the operator script of golem-cloud, as the admin database user. The billing
app has no command for it, and reads only `usage_settlements`. The script refuses while the
account has a billable subscription. It sets `usage_settlements.settled_through` of the account,
which records that Polar has billed, or will never bill, the usage events that the push recorded
until then, and adds a row with the same time, the operator and the reason to
`usage_settlement_history`. The sweep then removes the hold within a minute, and the cleanup does
not count these events either. A later event counts again.

Do not remove the hold of the app in the registry instead (`override-deletion-hold`): the next
reconcile places it again while the reason holds. After a delete, the cleanup checks the same
reasons (see [Deleted accounts](#deleted-accounts)): it cancels a subscription that is not
canceled at the period end at once, and for each other reason it keeps the user, so the person
cannot sign up again until the reason clears.

The registry refuses `DELETE /v1/accounts/{account_id}` with 409 while the account has a hold, and
shows the reason text to the person, for example in `golem-cli account delete`. The text starts
with `Golem Cloud billing:` and has one sentence for each reason that tells the person what to do:
cancel the subscription and wait for the end of the period, wait for the invoice, or pay it. Only
a billable subscription has an invoice to come, so the text of `unsettledUsage` promises none: it
says to try again in an hour, and then to contact Golem Cloud at `sales@golem.cloud`. The text of
`openCheckout` names the time after which the delete works: 30 minutes after the expiry of the
open checkout that expires last. After that time, while Polar has not expired the checkout, it says
to try again in an hour, and then to contact Golem Cloud. The text of `paidCheckout` says the
same. The rest of the paid period is not refunded.

## Billable overage

`node main.mjs push-usage` sends the billable overage to the meters of Polar once and exits, with
a nonzero exit code when it fails. A CronJob runs it every 10 minutes; the 15 minutes of
[Settlement](#settlement) need an interval of at most 10 minutes. It needs only the
`GOLEM__DB__*` variables and the Polar variables.

The billable overage of an account and month is the usage that a policy revision in
`allowOverage` received above the monthly amount. The registry reports it as `allowOverageUsage`
of `GET /v1/accounts/{account_id}/limits`, but only for the current month. The app calculates it
in the same way from the metering tables (`src/BillableUsage.ts` follows `apply_billable_excess`
of the registry), so it can also read a month that has closed:

| Table | Content |
| --- | --- |
| `account_monthly_policy_revisions` | the monthly policy revisions: mode, usage baselines and monthly amounts |
| `account_monthly_policy_attribution` | the usage that each policy revision received |

Each run reads the previous and the current UTC month, so usage that the registry receives for a
month after its last run in that month still reaches Polar. The table `usage_events` keeps one
event for every increase: it goes from the amount that earlier events submitted to the current
billable overage, in millionths of GCU, GB-seconds or GB-month. Polar refuses a numeric metadata
value above 2^53 - 1, so an increase above that becomes several events, each with an ID that ends
in the amount that it reaches. A run records its events first and then sends every event that is
not sent yet. Polar skips an event whose `external_id` it has
already, so after a failed send the next run sends the same events again and nothing twice. A
billable overage that goes down, for example after a refund, sends nothing.

Before it sends, a run holds the unsent events of each account without a billable subscription in
`polar_subscriptions`, or with a subscription past its scheduled end. Polar would accept them and
not bill them. A held event is never sent: the run logs the warning `unbilled_usage` with the
account, the meter and the amount, and `usage_events` keeps the event with `held_at`. The usage
of an account on an internal plan with overage is held in the same way.

| Event name | Unit |
| --- | --- |
| `golem.compute` | GCU |
| `golem.memory` | GB-seconds |
| `golem.durable_storage` | GB-month |
| `golem.ephemeral_storage` | GB-month |

Each event carries `units` (the increase in the unit of the meter), `micro_units` (the same
increase as an integer) and `period` (`YYYY-MM`). The Polar meters sum `units`.

A failed run, also one with a missing setting, logs one JSON line with the cause and exits with
code 1.

## Deleted accounts

`node main.mjs cleanup-deleted-accounts` runs once and exits, with a nonzero exit code when a call
to Polar fails. A CronJob runs it. It needs only the `GOLEM__DB__*` variables and the Polar
variables, with a Polar token that has only `customers:write`.

For each account in `user.golem_account_id`, `polar_subscriptions` or `polar_checkouts` that the
registry deleted, it:

1. checks the reasons of [Deletion hold](#deletion-hold) again, with the subscriptions and orders
   of every product. The hold makes them false before the delete, but an administrator can remove
   the hold.
   - A billable subscription that is not canceled at the period end (`subscription`) would charge
     the person again for an account that does not exist. The cleanup deletes the Polar customer at
     once, and logs the error `deleted_account_subscription_canceled` with the subscriptions: an
     operator refunds what the person paid for the time after the delete. Golem loses the paid
     overage that Polar has not billed.
   - With any other reason, the account keeps its Polar customer and its user, and each run logs the
     warning `deleted_account_unsettled`. A subscription that is canceled at the period end runs to
     its end, and an unfinished checkout waits until Polar expires it or its subscription comes;
2. deletes the Polar customer with `DELETE /v1/customers/external/{external_id}`. Polar cancels
   its subscriptions at once, with no refund, and clears its external ID. A 404 counts as deleted.
   The table `deleted_account_cleanups` records the time of the delete, so later runs do not call
   Polar for the account again;
3. deletes the Better Auth user of the account, with its sessions and GitHub accounts, and the
   row of the account in `deletion_holds`.

A run also checks the accounts that `deleted_account_cleanups` records. When the app recorded a
billable subscription that is not canceled at the period end after the delete of the customer,
Polar created a new customer with the external ID for it, for example for a checkout that the app
did not create. The cleanup deletes that customer too, logs `deleted_account_subscription_canceled`,
and records the new time. The cleanup does steps 1 and 2 of an account under the lock of the
account, as the subscription webhooks do. So the app records a subscription either before step 1
reads it, or after the recorded time of the delete, and a later run finds it.

The subscriptions, orders and usage events of the account stay. A person who signs up again with
the same GitHub identity gets a new Golem account at once. The billing app refuses its sign-in
until the cleanup has deleted the user of the deleted account (see
[Identity bridge](#identity-bridge)), which takes up to one interval of the CronJob.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `GOLEM__DB__CONFIG__HOST` | | PostgreSQL host. Same variable as the registry service. |
| `GOLEM__DB__CONFIG__PORT` | `5432` | PostgreSQL port. |
| `GOLEM__DB__CONFIG__DATABASE` | | Database name. |
| `GOLEM__DB__CONFIG__USERNAME` | | Database user. In the hosted deployment, the role of the workload, and the owner role for `migrate` (see [Database](#database)). |
| `GOLEM__DB__CONFIG__PASSWORD` | | Database password. |
| `GOLEM__DB__CONFIG__MAX_CONNECTIONS` | `10` | Size of the connection pool. |
| `GOLEM__DB__TYPE` | `Postgres` | Must be `Postgres` when set. |
| `GOLEM_CLOUD_BILLING_DB_SCHEMA` | `billing` | Schema of the app's tables. |
| `GOLEM_CLOUD_BILLING_REGISTRY_DB_SCHEMA` | `public` | Schema of the registry tables, for the metering and account tables. |
| `GOLEM_CLOUD_BILLING_DB_TLS` | `verify-full` | `disable`, `require` (TLS without certificate verification) or `verify-full`. `verify-full` also trusts the certificates in `NODE_EXTRA_CA_CERTS`, for example the Amazon RDS CA bundle. |
| `GOLEM_CLOUD_BILLING_HTTP_PORT` | `8080` | HTTP port. |
| `GOLEM_CLOUD_BILLING_REGISTRY_URL` | | Registry service API that the app calls, for example `http://registry-service:8081`. |
| `GOLEM_CLOUD_BILLING_PUBLIC_API_URL` | | Golem API URL for the CLI command on the landing page. With `https://release.api.golem.cloud`, the page shows `golem --cloud account get`. |
| `BETTER_AUTH_URL` | | Public origin of the app, for example `https://billing.golem.cloud`. |
| `BETTER_AUTH_SECRET` | | At least 32 characters. Signs cookies and encrypts tokens. |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | | GitHub OAuth app with the callback URL `<BETTER_AUTH_URL>/api/auth/callback/github`. |
| `GOLEM_CLOUD_BILLING_REGISTRY_TOKEN` | | Registry token of the billing account, which has `view`, `set-plan` and `hold-deletion` on every account through a permission share (see [Subscriptions](#subscriptions)). |
| `GOLEM_CLOUD_BILLING_FREE_PLAN_ID`, `GOLEM_CLOUD_BILLING_PAID_PLAN_ID`, `GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID` | | The plans that a subscription switches between (see [Subscriptions](#subscriptions)). |
| `GOLEM_CLOUD_BILLING_DEFAULT_PLAN_ID` | | Optional. The default plan of the registry (`initial_plans.default`), if the page offers Pro to an account on it (see [Subscriptions](#subscriptions)). It must differ from the three plans above. |
| `GOLEM_CLOUD_BILLING_PAID_<code>_PER_MONTH`, with `<code>` `USD`, `EUR` and `GBP` | | The monthly price of the paid plan in each currency, as a decimal number such as `20`: three variables. The page shows them next to the checkout (see [Currencies](#currencies)). Polar charges the fixed prices of the product, so these values must match the product. A change needs a restart, not a new image. |
| `GOLEM_CLOUD_BILLING_OVERAGE_<code>_PER_MILLION_GCU`, `GOLEM_CLOUD_BILLING_OVERAGE_<code>_PER_GB_HOUR`, `GOLEM_CLOUD_BILLING_OVERAGE_<code>_PER_DURABLE_GB_MONTH`, `GOLEM_CLOUD_BILLING_OVERAGE_<code>_PER_EPHEMERAL_GB_MONTH`, with `<code>` `USD`, `EUR` and `GBP` | | The overage prices of the paid plan in each currency, as decimal numbers such as `0.045`: twelve variables. The paid plan shows the prices of the currency of the subscription next to the paid-overage switch (see [Currencies](#currencies)). Polar bills with the metered prices of the product, so these values must match the product. A change needs a restart, not a new image. |
| `GOLEM_CLOUD_BILLING_PAST_DUE_GRACE` | `7 days` | How long an account keeps the paid plan after a payment of its subscription fails, as a duration such as `7 days` or `30 minutes` (see [Subscriptions](#subscriptions)). `0 seconds` turns the past-due grace off. Polar revokes the subscription after its last retry, 21 days after the failed payment, so a longer value has no effect. A test can use a short value, so that the past-due grace ends within the test. |
| `POLAR_API_URL` | | `https://sandbox-api.polar.sh` or `https://api.polar.sh`. |
| `POLAR_ACCESS_TOKEN` | | An organization access token of Polar. The web app needs `checkouts:write` (which also reads a checkout) and `customer_sessions:write`, `push-usage` needs `events:write`, and `cleanup-deleted-accounts` needs `customers:write`. Each gets its own token. |
| `POLAR_PRODUCT_ID` | | The Polar product of the paid plan. |
| `POLAR_SETTLEMENT_PRODUCT_ID` | | The Polar product of a settlement: a private one-time product with a fixed price in USD, which each checkout replaces with ad-hoc prices (see [Unpaid orders](#unpaid-orders)). It must differ from `POLAR_PRODUCT_ID` and have the same tax category. |
| `POLAR_WEBHOOK_SECRET` | | The secret that Polar signs the webhooks with (`whsec_...`). |

## Endpoints

| Path | Purpose |
| --- | --- |
| `GET /` | Landing page: the sign-in form, or the plan, the usage of the month, the account and the CLI command. |
| `POST /sign-in/github`, `POST /sign-out` | Form targets of the landing page. |
| `POST /billing/checkout`, `POST /billing/settle`, `POST /billing/portal` | Form targets of the plan: redirect to the checkout of the paid plan, the checkout of a settlement, or the customer portal of Polar. |
| `POST /billing/overage` | Form target of the paid-overage switch of the paid plan. It sets the monthly usage mode of the account (`allowOverage` or `hardLimit`) with the registry token of the session: the registry accepts paid overage only from the owner, and records the consent. The executors apply the change within 5 minutes, and the page says so. |
| `POST /api/auth/polar/webhooks` | The webhooks of Polar (see [Subscriptions](#subscriptions)). |
| `GET /api/auth/callback/github` | The GitHub OAuth callback of Better Auth. The other Better Auth endpoints are not routed; the pages call them on the server. |
| `GET /healthz` | Liveness. |
| `GET /readyz` | Readiness: returns 503 when the database is unreachable. |

The form targets answer 403 to a post whose `Origin` header is not the origin of `BETTER_AUTH_URL`,
also to a post without `Origin`. The webhook route of Polar does not check `Origin`.

Better Auth limits the requests to its handler for each client address, in the memory of each
replica. `POST /sign-in/github` goes through the handler: a client address starts at most 3 GitHub
sign-ins in 10 seconds, and the next one redirects to `/?error=too_many_requests`.

The client address is the last address of `x-forwarded-for`, or the address of the connection
when the request has no such header (`clientAddress` in `src/Http.ts`). The app assumes one proxy
hop: the gateway appends the address that it sees, and every address to the left of it comes from
the client. Before the app calls the handler of Better Auth, it sets `x-forwarded-for` to the
client address alone, because Better Auth trusts the header only when it has one address. The
checkout also sends this address to Polar, for the local currency. The gateway must see the address
of the client: on golem-dev the NLB of the gateway has instance targets and
`externalTrafficPolicy: Local`, and golem-prod must do the same.

## Development

Use Node.js 24 and pnpm.

```shell
pnpm install
pnpm typecheck
pnpm start migrate   # node src/main.ts migrate; Node.js strips the types
pnpm start
pnpm build           # bundles src/main.ts and its dependencies into dist/main.mjs
```

On a local database, create the schema (`CREATE SCHEMA billing`) before the first `migrate`.

The tests need a PostgreSQL server and read its address from the libpq variables. Each test file
creates its own schema and drops it afterwards. GitHub, the registry service API and Polar are
faked. The tests that read registry tables (billing, identity bridge, metering, usage push,
cleanup and migrations) create a database with all registry migrations of
`../golem-registry-service/db/migration/postgres`. They run `migrate` as the owner role, and each
workload as its own role with the grants of the hosted deployment, so the user needs CREATEDB and
CREATEROLE.

```shell
docker run -d --name billing-pg -e POSTGRES_PASSWORD=postgres -p 5432:5432 postgres:17-alpine
PGHOST=localhost PGUSER=postgres PGPASSWORD=postgres PGDATABASE=postgres pnpm test
```

## Image

`docker/Dockerfile` builds the bundle and copies it onto `gcr.io/distroless/nodejs24-debian13`,
which has no shell and no package manager. The image runs as uid 65534. Build it from this
directory:

```shell
docker build -f docker/Dockerfile -t golem-cloud-billing .
```

CI (`.github/workflows/golem-cloud-billing.yaml`) publishes `golemservices/golem-cloud-billing`
with a `main-<run number>-<short SHA>` tag for every change on `main`, and a `v<version>` tag for
every Golem release. To test a branch on a cluster before the merge, start the workflow on that
branch:

```shell
gh workflow run golem-cloud-billing.yaml --repo golemcloud/golem --ref <branch>
```

The run publishes a `test-<run number>-<short SHA>` tag. The image policy of Flux selects only the
`main` tags, so a test image runs only where a deployment pins its digest.
