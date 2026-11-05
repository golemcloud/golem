# golem-cloud-billing

The billing app of the hosted Golem Cloud. Users sign in with GitHub, subscribe to the paid plan
through Polar.sh, and manage the subscription in the customer portal of Polar. Self-hosted Golem
does not use it.

The app calls the HTTP API of the registry service, and reads three metering tables of the
registry (see [Billable overage](#billable-overage)). It uses nothing else in Golem, and the
registry knows nothing about the app. In the hosted deployment both use the same PostgreSQL
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
and a Golem account other than the one in `user.golem_account_id`.

## Database

The connections of the app set `search_path` to its schema (`billing` by default), and Better Auth
creates its tables there. On startup the app applies the Better Auth migrations under a PostgreSQL
advisory lock, so that replicas migrate one at a time.

In the hosted deployment the app connects with its own role, which owns its schema. The registry
tables hold API token secrets and agent secrets, so the role can read only the three metering
tables of the registry. Everything else comes from the registry HTTP API. As the signed-in user,
the app calls `POST /v1/login/oauth2` at sign-in and `GET /v1/accounts/{account_id}` for the
account page. As the billing account, the webhooks call `GET /v1/accounts/{account_id}` and
`PUT /v1/accounts/{account_id}/plan` (see [Subscriptions](#subscriptions)). The app also keeps
two tables of its own: `polar_subscriptions` and `polar_webhook_deliveries`. An administrator
creates the role once:

```sql
CREATE ROLE golem_cloud_billing LOGIN PASSWORD '…' CONNECTION LIMIT 10;
CREATE SCHEMA billing AUTHORIZATION golem_cloud_billing;
GRANT CONNECT ON DATABASE <database> TO golem_cloud_billing;
GRANT USAGE ON SCHEMA public TO golem_cloud_billing;
GRANT SELECT ON public.account_usage_stats, public.account_monthly_policy_revisions,
  public.account_monthly_policy_attribution TO golem_cloud_billing;
```

When the schema does not exist, the app creates it, which needs the CREATE privilege on the
database. A local database where the app connects as its owner works without any setup.

Better Auth reads the columns of every table in the database when the app starts. When a registry
migration changes a table at the same moment, the read fails; the app tries it again up to five
times.

## Subscriptions

A subscription of the Polar product `POLAR_PRODUCT_ID` moves the Golem account between three
plans. The Polar customer has the Golem account ID as its external ID.

| Plan | Variable | When |
| --- | --- | --- |
| paid | `GOLEM_CLOUD_BILLING_PAID_PLAN_ID` | an `active` subscription that does not end within 15 minutes |
| suspended | `GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID` | the account owes a payment: a `past_due` or `unpaid` subscription, or one that Polar revoked while it was unpaid |
| free | `GOLEM_CLOUD_BILLING_FREE_PLAN_ID` | otherwise: no subscription, a cancellation, a trial, a pause |

The paid plan is prepaid with optional postpay: its price buys the monthly amounts in advance,
and paid overage is billed after the period. Only the paid plan allows overage. The suspended plan
has no monthly amounts and `overage_eligible: false`, so the agents of the account stop, and an
account that owes a payment gets no free usage. On a plan without overage the registry sets the
account to `hardLimit`. Only the owner can turn paid overage on again
(`PUT /v1/accounts/{account_id}/monthly-usage-mode`), also after the account is back on the paid
plan.

- **Checkout.** `POST /billing/checkout` creates a checkout session for the signed-in user and
  redirects to it: the paid product, the Golem account ID as `external_customer_id`, the email and
  the name of the user, and the IP address of the browser for the local currency. After the
  payment Polar redirects to `/?checkout_id=...`, and the page shows that the plan is activating
  until the webhook changes it.
- **Customer portal.** `POST /billing/portal` creates a customer session for the Golem account and
  redirects to the customer portal of Polar: payment method, invoices, cancellation.
- **Webhooks.** Polar delivers `subscription.*` events to `POST /api/auth/polar/webhooks`. The
  handler verifies the Standard Webhooks signature with `POLAR_WEBHOOK_SECRET` (403 when it does
  not match), skips a `webhook-id` that it handled before, and stores the subscription in
  `polar_subscriptions`. A state never replaces a state with a later `modified_at`, so a late
  delivery cannot undo a newer one. Then it sets the plan from the table above. A failed payment
  (`subscription.past_due`) also logs `payment_failed`. Polar revokes a subscription that stays
  unpaid as `canceled`, like a cancellation, so the table keeps `past_due_at` from the first
  `past_due` state until an `active` state. An account on any other plan, for example an internal
  plan that an administrator gave it, keeps it.
- **Sweep.** A subscription that is canceled at the period end stays `active` until Polar revokes
  it. Every minute, each replica of the web app moves the accounts whose subscription ends within
  15 minutes to the free plan.
- **Order and retries.** An advisory lock per account serializes the webhooks and the sweep of one
  account, also across replicas. When the registry or the database fails, the handler answers 500 and Polar
  delivers the webhook again. A 409 of the registry (the account changed between the read and the
  write) makes the handler read the account again.

The plan of a customer is not up to its owner: the registry refuses `set-plan` on the own account.
The app changes plans with the token of a separate billing account
(`GOLEM_CLOUD_BILLING_REGISTRY_TOKEN`). An administrator gives that account the two permissions
once, with a permission share from the administrator account
(`POST /v1/accounts/<administrator account ID>/permission-shares` with an administrator token):

```json
{
  "targetAccountEmail": "<email of the billing account>",
  "name": "cloud-billing-set-plan",
  "data": {
    "lowerPositive": [
      "account(*) @ <email of the billing account> : view :",
      "account(*) @ <email of the billing account> : set-plan :"
    ],
    "lowerNegative": [],
    "upperPositive": [],
    "upperNegative": []
  }
}
```

`set-plan` needs `view`, because the registry reads the account before it changes the plan. The
token of the billing account can change plans and read accounts, and nothing else.

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
- **Failed payment.** The account is suspended while it owes the payment, also after Polar
  revokes the subscription. A payment of the same subscription, or a new subscription, moves it
  back to the paid plan.
- **Trial.** A trial gives the free plan. The paid product has no trial.
- **Product change.** The app has one paid product, so a subscription does not change its
  product. Polar invoices the usage of the old product at the change; a second product needs a
  push before that change.

The usage push does not send an event that Polar would not bill (see
[Billable overage](#billable-overage)).

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
| `account_usage_stats` | the usage of each account and month |
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

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `GOLEM__DB__CONFIG__HOST` | | PostgreSQL host. Same variable as the registry service. |
| `GOLEM__DB__CONFIG__PORT` | `5432` | PostgreSQL port. |
| `GOLEM__DB__CONFIG__DATABASE` | | Database name. |
| `GOLEM__DB__CONFIG__USERNAME` | | Database user. In the hosted deployment, the role of the app (see [Database](#database)). |
| `GOLEM__DB__CONFIG__PASSWORD` | | Database password. |
| `GOLEM__DB__CONFIG__MAX_CONNECTIONS` | `10` | Size of the connection pool. |
| `GOLEM__DB__TYPE` | `Postgres` | Must be `Postgres` when set. |
| `GOLEM_CLOUD_BILLING_DB_SCHEMA` | `billing` | Schema of the app's tables. |
| `GOLEM_CLOUD_BILLING_REGISTRY_DB_SCHEMA` | `public` | Schema of the registry tables, for the metering tables. |
| `GOLEM_CLOUD_BILLING_DB_TLS` | `verify-full` | `disable`, `require` (TLS without certificate verification) or `verify-full`. `verify-full` also trusts the certificates in `NODE_EXTRA_CA_CERTS`, for example the Amazon RDS CA bundle. |
| `GOLEM_CLOUD_BILLING_HTTP_PORT` | `8080` | HTTP port. |
| `GOLEM_CLOUD_BILLING_REGISTRY_URL` | | Registry service API that the app calls, for example `http://registry-service:8081`. |
| `GOLEM_CLOUD_BILLING_PUBLIC_API_URL` | | Golem API URL for the CLI command on the landing page. With `https://release.api.golem.cloud`, the page shows `golem --cloud account get`. |
| `BETTER_AUTH_URL` | | Public origin of the app, for example `https://billing.golem.cloud`. |
| `BETTER_AUTH_SECRET` | | At least 32 characters. Signs cookies and encrypts tokens. |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | | GitHub OAuth app with the callback URL `<BETTER_AUTH_URL>/api/auth/callback/github`. |
| `GOLEM_CLOUD_BILLING_REGISTRY_TOKEN` | | Registry token of the billing account, which has `view` and `set-plan` on every account through a permission share (see [Subscriptions](#subscriptions)). |
| `GOLEM_CLOUD_BILLING_FREE_PLAN_ID`, `GOLEM_CLOUD_BILLING_PAID_PLAN_ID`, `GOLEM_CLOUD_BILLING_SUSPENDED_PLAN_ID` | | The plans that a subscription switches between (see [Subscriptions](#subscriptions)). |
| `POLAR_API_URL` | | `https://sandbox-api.polar.sh` or `https://api.polar.sh`. |
| `POLAR_ACCESS_TOKEN` | | An organization access token of Polar. The web app needs `checkouts:write` and `customer_sessions:write`; `push-usage` needs `events:write`. Each gets its own token. |
| `POLAR_PRODUCT_ID` | | The Polar product of the paid plan. |
| `POLAR_WEBHOOK_SECRET` | | The secret that Polar signs the webhooks with (`whsec_...`). |

## Endpoints

| Path | Purpose |
| --- | --- |
| `GET /` | Landing page: the sign-in form, or the plan, the account and the CLI command. |
| `POST /sign-in/github`, `POST /sign-out` | Form targets of the landing page. |
| `POST /billing/checkout`, `POST /billing/portal` | Form targets of the plan: redirect to the checkout or the customer portal of Polar. |
| `POST /api/auth/polar/webhooks` | The webhooks of Polar (see [Subscriptions](#subscriptions)). |
| `GET /api/auth/callback/github` | The GitHub OAuth callback of Better Auth. The other Better Auth endpoints are not routed; the pages call them on the server. |
| `GET /healthz` | Liveness. |
| `GET /readyz` | Readiness: returns 503 when the database is unreachable. |

## Development

Use Node.js 24 and pnpm.

```shell
pnpm install
pnpm typecheck
pnpm start   # node src/main.ts; Node.js strips the types
pnpm build   # bundles src/main.ts and its dependencies into dist/main.mjs
```

The tests need a PostgreSQL server and read its address from the libpq variables. Each test file
creates its own schema and drops it afterwards. GitHub, the registry service API and Polar are
faked. The metering tests create a database with all registry migrations of
`../golem-registry-service/db/migration/postgres`, so the user needs CREATEDB and CREATEROLE.

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
every Golem release.
