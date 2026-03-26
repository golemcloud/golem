---
name: golem-db-app
description: "Building a Golem application with PostgreSQL database integration. Use when creating agents that store and query data using golem:rdbms/postgres, defining HTTP endpoints, and configuring environment variables."
---

# Building a Database-Backed Golem Application

**Important: Do not try to build golem from scratch or install it manually.**

Assume the `golem` or `golem-cli` binary exists and is added to PATH.
Try `golem --version` to check if it exists. If not, try `golem-cli --version`.

**Critical rules:**
- You must create the application with `golem new <APP_NAME> --template ts -Y`. Do not hand-write `golem.yaml`, `package.json`, `tsconfig.json`, or the initial source tree from scratch.
- Do NOT modify SDK versions in `package.json`. The SDK is resolved automatically via local paths. Changing versions to npm-published ones will break the build.
- Do NOT remove or modify `@agent`, `@endpoint`, `@prompt`, or `@description` decorators. They are valid and required.
- Do NOT run `npm install` after `golem new` — dependencies are already set up correctly, and replacing them can break `golem build`.
- Use `ls` (shell command) to check SDK types in `node_modules/`, not `read_file` — some tools block reading inside `node_modules`.
- Start from the generated TypeScript scaffold. Inspect `src/main.ts`, the generated agent file, and `golem.yaml` before editing them.
- If you replace the generated `CounterAgent`, also remove or rename the old file so build metadata does not keep tracking both agents.
- A successful `golem build` is not enough for this skill. When the task includes deployment, verify the HTTP route works after deploy.

## Step 1: Create the Project

```shell
golem new <APP_NAME> --template ts -Y
```

Do this even if the current workspace is empty. The generated app directory is the source of truth for the project layout. After scaffolding, switch into that directory before making edits:

```shell
cd <APP_NAME>
```

Then inspect the generated files before changing them:

```shell
ls src
sed -n '1,200p' src/main.ts
sed -n '1,240p' src/counter-agent.ts
sed -n '1,240p' golem.yaml
```

## Step 2: Determine the rdbms Module Version

Check which rdbms version the installed SDK provides:

```shell
ls node_modules/@golemcloud/golem-ts-sdk/types/ | grep rdbms
```

This will show files like `golem_rdbms_X_Y_Z_postgres.d.ts`. Use that version in your import (e.g., `@1.5.0` or `@0.0.2`).
If needed, inspect the matching `.d.ts` file too so the imports and result/value handling match the installed SDK.

## Step 3: Convert the Generated Agent to a PostgreSQL Agent

Replace the generated counter example with your database-backed agent inside the scaffolded app. Keep the same decorator style as the scaffold and use the rdbms version found in Step 2.

Do not create a custom app skeleton or swap in a different package layout. The goal is to edit the generated project, not recreate it.

Use this shape:

```typescript
import {
  BaseAgent,
  agent,
  prompt,
  description,
  endpoint
} from '@golemcloud/golem-ts-sdk';
import { DbConnection, DbValue, DbResult } from 'golem:rdbms/postgres@X.Y.Z';

@agent({
  mount: "/items/{name}"
})
export class ItemAgent extends BaseAgent {
  private readonly name: string;
  private readonly dbUrl: string;
  private initialized: boolean = false;

  constructor(name: string) {
    super();
    this.name = name;

    const dbUrl = process.env.DB_POSTGRES_URL;
    if (!dbUrl) {
      throw new Error('DB_POSTGRES_URL is not set');
    }

    this.dbUrl = dbUrl;
  }

  private getDb(): DbConnection {
    return DbConnection.open(this.dbUrl);
  }

  private ensureTable(): void {
    if (this.initialized) {
      return;
    }

    const db = this.getDb();
    db.execute(
      "CREATE TABLE IF NOT EXISTS items (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
      []
    );
    this.initialized = true;
  }

  @prompt("Add an item with a name")
  @description("Inserts an item into PostgreSQL and returns a confirmation message")
  @endpoint({ post: "/add" })
  async addItem(name: string): Promise<string> {
    this.ensureTable();
    const db = this.getDb();
    db.execute(
      "INSERT INTO items (name) VALUES ($1)",
      [{ tag: 'text', val: name }]
    );
    return `Added: ${name}`;
  }

  @prompt("List all items")
  @description("Returns all item names from PostgreSQL")
  @endpoint({ get: "/list" })
  async listItems(): Promise<string[]> {
    this.ensureTable();
    const db = this.getDb();
    const result: DbResult = db.query("SELECT name FROM items ORDER BY id", []);

    return result.rows.map((row) => {
      const val = row.values[0];
      if (!val || val.tag === 'null') {
        return '';
      }
      if (val.tag === 'text' || val.tag === 'varchar' || val.tag === 'bpchar') {
        return val.val;
      }
      return String(val.val);
    });
  }
}
```

### DbValue Tagged Union

All query parameters use the `DbValue` tagged union. Common tags:

| TypeScript | DbValue |
|-----------|---------|
| `string` | `{ tag: 'text', val: "hello" }` |
| `number` (int) | `{ tag: 'int4', val: 42 }` |
| `number` (float) | `{ tag: 'float8', val: 3.14 }` |
| `boolean` | `{ tag: 'boolean', val: true }` |
| `bigint` | `{ tag: 'int8', val: 100n }` |
| `null` | `{ tag: 'null' }` |

### DbResult Structure

```typescript
const result: DbResult = db.query("SELECT id, name FROM items", []);
// result.columns: DbColumn[] — column metadata
// result.rows: DbRow[] — array of rows
// result.rows[0].values: DbValue[] — values in column order
```

### Transactions

```typescript
const db = this.getDb();
const tx = db.beginTransaction();
try {
    tx.execute("INSERT INTO items (name) VALUES ($1)", [{ tag: 'text', val: "a" }]);
    tx.execute("INSERT INTO items (name) VALUES ($1)", [{ tag: 'text', val: "b" }]);
    tx.commit();
} catch (e) {
    tx.rollback();
    throw e;
}
```

## Step 4: Export the Agent

In `src/main.ts`:

```typescript
export { ItemAgent } from './item-agent';
```

If the scaffold still has `counter-agent.ts` and you are replacing it entirely, delete or rename it and make sure `src/main.ts` only exports the new database-backed agent.

## Step 5: Configure Environment Variables

In `golem.yaml`, add the database URL under the component's `env` section:

```yaml
components:
  <app-name>:ts-main:
    env:
      DB_POSTGRES_URL: "postgres://postgres:postgres@localhost:5432/golem_test"
```

The component name follows the pattern `<app-name>:ts-main` for single-component TypeScript apps.

## Step 6: Build and Deploy

```shell
cd <APP_NAME>
golem build
golem deploy --yes
```

After deployment, HTTP endpoints are available at `http://<app-name>.localhost:9006/<mount-path>/<endpoint-path>`.

Do not replace the generated dependencies with a manual `npm install` flow before `golem build`. `golem new` already created the correct project for `golem build`.

If `golem build` output still mentions the old scaffold agent as tracked metadata, remove the stale file and rebuild from a clean state:

```shell
golem clean
golem build
```

For deployed apps, smoke test the real HTTP flow before claiming success. Example:

```shell
curl -sS -X POST http://<app-name>.localhost:9006/items/default/add \
  -H 'Content-Type: application/json' \
  -d '{"name":"test item"}'

curl -sS http://<app-name>.localhost:9006/items/default/list
```

## Checklist

1. `golem new` executed successfully
2. Agent imports `DbConnection` from `'golem:rdbms/postgres@X.Y.Z'` (version from Step 2)
3. Agent preserves `@agent`, `@endpoint`, `@prompt`, and `@description` decorators
4. `DB_POSTGRES_URL` set in `golem.yaml` under component env
5. Work happened inside the scaffolded `<APP_NAME>/` directory rather than a hand-built project root
6. `src/main.ts` exports the new agent and the old scaffold agent is removed if no longer used
7. `golem build` succeeds
8. If needed, `golem clean && golem build` leaves only the intended agent metadata
9. `golem deploy --yes` succeeds
10. The deployed HTTP endpoint returns a successful response for a real insert/list smoke test
