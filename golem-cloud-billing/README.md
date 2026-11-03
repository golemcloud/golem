# golem-cloud-billing

The billing app of the hosted Golem Cloud. Users sign in with GitHub, see their plan, and (later)
manage paid plans through Polar.sh. Self-hosted Golem does not use it.

The app calls the HTTP API of the registry service and nothing else in Golem. The registry knows
nothing about the app. In the hosted deployment both use the same PostgreSQL database: the
registry keeps its tables in `public`, and the app keeps its tables in its own schema.

## Stack

- [Effect 4](https://effect.website) for the HTTP server, configuration, services and the
  registry client.
- [Better Auth](https://better-auth.com) with the GitHub provider, mounted on the Effect router
  as described in its [Effect integration](https://better-auth.com/docs/integrations/effect).
  The Polar.sh plugin of Better Auth plugs into the same handler.
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

## Database

The connections of the app set `search_path` to its schema (`billing` by default), and Better Auth
creates its tables there. On startup the app applies the Better Auth migrations under a PostgreSQL
advisory lock, so that replicas migrate one at a time.

In the hosted deployment the app connects with its own role, which owns its schema and nothing
else. The registry tables hold API token secrets and agent secrets, and the app reads registry
data only through the HTTP API. An administrator creates the role once:

```sql
CREATE ROLE golem_cloud_billing LOGIN PASSWORD '…' CONNECTION LIMIT 10;
CREATE SCHEMA billing AUTHORIZATION golem_cloud_billing;
GRANT CONNECT ON DATABASE <database> TO golem_cloud_billing;
```

When the schema does not exist, the app creates it, which needs the CREATE privilege on the
database. A local database where the app connects as its owner works without any setup.

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
| `GOLEM_CLOUD_BILLING_DB_TLS` | `verify-full` | `disable`, `require` (TLS without certificate verification) or `verify-full`. `verify-full` also trusts the certificates in `NODE_EXTRA_CA_CERTS`, for example the Amazon RDS CA bundle. |
| `GOLEM_CLOUD_BILLING_HTTP_PORT` | `8080` | HTTP port. |
| `GOLEM_CLOUD_BILLING_REGISTRY_URL` | | Registry service API that the app calls, for example `http://registry-service:8081`. |
| `GOLEM_CLOUD_BILLING_PUBLIC_API_URL` | | Golem API URL for the CLI command on the landing page. With `https://release.api.golem.cloud`, the page shows `golem --cloud account get`. |
| `BETTER_AUTH_URL` | | Public origin of the app, for example `https://billing.golem.cloud`. |
| `BETTER_AUTH_SECRET` | | At least 32 characters. Signs cookies and encrypts tokens. |
| `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET` | | GitHub OAuth app with the callback URL `<BETTER_AUTH_URL>/api/auth/callback/github`. |

## Endpoints

| Path | Purpose |
| --- | --- |
| `GET /` | Landing page: the sign-in form, or the plan, the account and the CLI command. |
| `POST /sign-in/github`, `POST /sign-out` | Form targets of the landing page. |
| `/api/auth/*` | Better Auth. |
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
creates its own schema and drops it afterwards. GitHub and the registry service are faked.

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
