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
`polar_subscriptions`, `polar_orders`, `polar_webhook_deliveries`, `usage_events`,
`deleted_account_cleanups`, `deletion_holds`, `usage_settlements` and `usage_settlement_history`.

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
  `polar_subscriptions` and `polar_orders`; SELECT, INSERT, UPDATE and DELETE on `session` and
  `deletion_holds`; SELECT, INSERT and DELETE on `verification`; SELECT and INSERT on
  `polar_webhook_deliveries`; SELECT on `usage_events` and `usage_settlements`; SELECT
  (`account_id`, `deleted_at`) on `public.accounts`.
- `golem_cloud_billing_push`, for `push-usage`: SELECT, INSERT and UPDATE on `usage_events`;
  SELECT on `polar_subscriptions`, `public.account_monthly_policy_revisions` and
  `public.account_monthly_policy_attribution`.
- `golem_cloud_billing_cleanup`, for `cleanup-deleted-accounts`: SELECT (`golem_account_id`) and
  DELETE on `user`; SELECT and INSERT on `deleted_account_cleanups`; SELECT and DELETE on
  `deletion_holds`; SELECT on `polar_subscriptions`, `polar_orders`, `usage_events` and
  `usage_settlements`; SELECT (`account_id`, `deleted_at`) on `public.accounts`. The ON DELETE
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
the billing account, the webhooks and the sweep call `GET /v1/me/token`,
`GET /v1/accounts/{account_id}`, `PUT /v1/accounts/{account_id}/plan`, and `PUT` and `DELETE`
`/v1/accounts/{account_id}/deletion-holds/{holder_account_id}` (see
[Subscriptions](#subscriptions)).

## Subscriptions

A subscription of the Polar product `POLAR_PRODUCT_ID` and its orders move the Golem account
between three plans. The Polar customer has the Golem account ID as its external ID.

| Plan | Variable | When |
| --- | --- | --- |
| suspended | `GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID` | paid overage is not paid: a `pending` order has an amount for a metered price, its subscription is not in the past-due grace, and a payment of the subscription failed or the order is older than one hour |
| paid | `GOLEM_CLOUD_BILLING_PAID_PLAN_ID` | an `active` subscription that does not end within 15 minutes, or a `past_due` subscription in the past-due grace |
| free | `GOLEM_CLOUD_BILLING_FREE_PLAN_ID` | otherwise: no subscription, a declined payment of the price alone after the past-due grace, a cancellation, a trial, a pause |

The paid plan is prepaid with optional postpay. Its price buys the monthly amounts in advance.
When a payment fails, the account keeps the paid plan for the past-due grace. When the payment of
the price is still open after the past-due grace, the account is on the free plan until the
payment, as without a subscription. Paid overage is billed after the period, for resources that
the account has used: when that payment is still open after the past-due grace, the account is
suspended until the order is paid, also with a new subscription. The suspended plan has no monthly amounts and `overage_eligible: false`, so the
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
  until the webhook changes it.
- **Customer portal.** `POST /billing/portal` creates a customer session for the Golem account and
  redirects to the customer portal of Polar: payment method, invoices, cancellation.
- **Webhooks.** Polar delivers `subscription.*`, `order.created`, `order.updated` and `order.paid`
  events to `POST /api/auth/polar/webhooks`. The handler verifies the Standard Webhooks signature
  with `POLAR_WEBHOOK_SECRET` (403 when it does not match), skips a `webhook-id` that it handled
  before, and stores the subscription in `polar_subscriptions` or the order in `polar_orders`. A
  state never replaces a state with a later `modified_at`, so a late delivery cannot undo a newer
  one. Then it sets the [deletion hold](#deletion-hold) and the plan from the table above. A failed
  payment (`subscription.past_due`) also logs `payment_failed`. An account on any other plan, for
  example an internal plan that an administrator gave it, keeps it, and gets the deletion hold all
  the same. An event of an account that the registry deleted is stored and answers 200, without a
  change of the plan or the hold, so that deleted accounts do not make Polar disable the endpoint.
- **Metered prices.** An order item names its price. The subscription lists the prices of its
  product, so `polar_subscriptions` keeps the IDs of the metered prices, and the handler counts
  the items of these prices as overage.
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
  past-due grace ends, and usage events that the usage push recorded, sent or held in the last 24
  hours for an account without a subscription that continues.
- **Order and retries.** An advisory lock per account serializes the webhooks and the sweep of one
  account, also across replicas. When the registry or the database fails, the handler answers 500 and Polar
  delivers the webhook again. A 404 of the registry for an account that is not deleted also
  answers 500: the permission share of the billing account is missing. A 409 of the registry (the
  account changed between the read and the write) makes the handler read the account again.

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

### Deletion hold

Golem does not charge early, and a cancellation runs to the end of the period: Polar bills paid
overage on the order at the end of the period. So the registry must not delete an account while
the account has a billable subscription or owes paid overage. The app places a deletion hold on
the account through the registry (`Settlement.holdFor`) while any of these is true, and removes it
when none is:

| Reason | When |
| --- | --- |
| `subscription` | a billable subscription (`active`, `trialing` or `past_due`) is not canceled at the period end |
| `canceledSubscription` | a billable subscription is canceled at the period end and runs to the end of the period |
| `unbilledUsage` | a billable subscription, and a usage event that no order bills yet |
| `unsettledUsage` | no billable subscription, and a usage event that no order bills yet |
| `unpaidOverage` | a `pending` order has an amount for a metered price |

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

An unpaid order holds the account as `unpaidOverage`, also after an early end, because the
customer can still pay it.

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
It also reconciles the accounts without a billable subscription whose usage events changed, because
no webhook comes for a usage event, and each account whose row is older than an hour
(`HOLD_RECHECK_MS`). A placement refreshes the row, and a removal that failed keeps it, so the
sweep tries the removal again with no time limit.

An `unsettledUsage` hold clears by itself within minutes: the push sends or holds an unsent event
at its next run, and Polar creates the last order soon after the period end. When it stays, for
example because the last order never comes, an operator checks the account in Polar and records a
usage settlement with the operator script of golem-cloud, as the admin database user. The billing
app has no command for it, and reads only `usage_settlements`. The script refuses while the
account has a billable subscription. It sets `usage_settlements.settled_through` of the account,
which records that Polar has billed, or will never bill, the usage events that the push recorded
until then, and adds a row with the same time, the operator and the reason to
`usage_settlement_history`. The sweep then removes the hold within an hour (`HOLD_RECHECK_MS`),
and the cleanup does not count these events either. A later event counts again.

Do not remove the hold of the app in the registry instead (`override-deletion-hold`): the next
reconcile places it again while the reason holds, and after a delete the cleanup checks the same
reasons and keeps the user, so the person cannot sign up again.

The registry refuses `DELETE /v1/accounts/{account_id}` with 409 while the account has a hold, and
shows the reason text to the person, for example in `golem-cli account delete`. The text starts
with `Golem Cloud billing:` and has one sentence for each reason that tells the person what to do:
cancel the subscription and wait for the end of the period, wait for the invoice, or pay it. Only
a billable subscription has an invoice to come, so the text of `unsettledUsage` promises none: it
says to try again in an hour, and then to contact support at `contact@golem.cloud`. The rest of
the paid period is not refunded.

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

For each account in `user.golem_account_id` or `polar_subscriptions` that the registry deleted, it:

1. checks the reasons of [Deletion hold](#deletion-hold) again, with the subscriptions and orders
   of every product. The hold makes them false before the delete, but an administrator can remove
   the hold. An account with a reason keeps its Polar customer and its user, and each run logs the
   warning `deleted_account_unsettled`, so the cleanup never ends a paid period early;
2. deletes the Polar customer with `DELETE /v1/customers/external/{external_id}`. Polar cancels
   its subscriptions, which have all ended by now, and clears its external ID. A 404 counts as
   deleted. The table `deleted_account_cleanups` records the account, so later runs do
   not call Polar for it again;
3. deletes the Better Auth user of the account, with its sessions and GitHub accounts, and the
   row of the account in `deletion_holds`.

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
| `GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_MILLION_GCU`, `GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_GB_HOUR`, `GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_DURABLE_GB_MONTH`, `GOLEM_CLOUD_BILLING_OVERAGE_USD_PER_EPHEMERAL_GB_MONTH` | | The overage prices that the paid plan shows next to the paid-overage switch, in USD, as decimal numbers such as `0.045`. Polar bills with the metered prices of the product, so these values must match the product. A change needs a restart, not a new image. |
| `GOLEM_CLOUD_BILLING_PAST_DUE_GRACE` | `7 days` | How long an account keeps the paid plan after a payment of its subscription fails, as a duration such as `7 days` or `30 minutes` (see [Subscriptions](#subscriptions)). `0 seconds` turns the past-due grace off. Polar revokes the subscription after its last retry, 21 days after the failed payment, so a longer value has no effect. A test can use a short value, so that the past-due grace ends within the test. |
| `POLAR_API_URL` | | `https://sandbox-api.polar.sh` or `https://api.polar.sh`. |
| `POLAR_ACCESS_TOKEN` | | An organization access token of Polar. The web app needs `checkouts:write` and `customer_sessions:write`, `push-usage` needs `events:write`, and `cleanup-deleted-accounts` needs `customers:write`. Each gets its own token. |
| `POLAR_PRODUCT_ID` | | The Polar product of the paid plan. |
| `POLAR_WEBHOOK_SECRET` | | The secret that Polar signs the webhooks with (`whsec_...`). |

## Endpoints

| Path | Purpose |
| --- | --- |
| `GET /` | Landing page: the sign-in form, or the plan, the account and the CLI command. |
| `POST /sign-in/github`, `POST /sign-out` | Form targets of the landing page. |
| `POST /billing/checkout`, `POST /billing/portal` | Form targets of the plan: redirect to the checkout or the customer portal of Polar. |
| `POST /billing/overage` | Form target of the paid-overage switch of the paid plan. It sets the monthly usage mode of the account (`allowOverage` or `hardLimit`) with the registry token of the session: the registry accepts paid overage only from the owner, and records the consent. The executors apply the change within 5 minutes, and the page says so. |
| `POST /api/auth/polar/webhooks` | The webhooks of Polar (see [Subscriptions](#subscriptions)). |
| `GET /api/auth/callback/github` | The GitHub OAuth callback of Better Auth. The other Better Auth endpoints are not routed; the pages call them on the server. |
| `GET /healthz` | Liveness. |
| `GET /readyz` | Readiness: returns 503 when the database is unreachable. |

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
