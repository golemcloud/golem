---
name: golem-db-app
description: "Building a Golem application with PostgreSQL database integration. Use when creating agents that store and query data using golem:rdbms/postgres, defining HTTP endpoints, and configuring environment variables."
---

# Building a Database-Backed Golem Application

**Important: Do not try to build golem from scratch or install it manually.**

Assume the `golem` or `golem-cli` binary exists and is added to PATH.
Try `golem --version` to check if it exists. If not, try `golem-cli --version`.

## Step 1: Create the Project

```shell
golem new <APP_NAME> --template ts -Y
```

## Step 2: Determine the rdbms Module Version

Check which rdbms version the installed SDK provides:

```shell
ls node_modules/@golemcloud/golem-ts-sdk/types/ | grep rdbms
```

This will show files like `golem_rdbms_X_Y_Z_postgres.d.ts`. Use that version in your import (e.g., `@1.5.0` or `@0.0.2`).

## Step 3: Write the Agent with PostgreSQL

Import the database module and define an agent with HTTP endpoints. Use the rdbms version found in Step 2:

```typescript
import {
    BaseAgent,
    agent,
    prompt,
    description,
    endpoint
} from '@golemcloud/golem-ts-sdk';
// Use the version from Step 2 (check node_modules/@golemcloud/golem-ts-sdk/types/)
import { DbConnection, DbValue, DbResult } from 'golem:rdbms/postgres@X.Y.Z';

@agent({
    mount: "/items/{name}"
})
export class ItemAgent extends BaseAgent {
    private readonly name: string;
    private initialized: boolean = false;

    constructor(name: string) {
        super();
        this.name = name;
    }

    private getDb(): DbConnection {
        return DbConnection.open(process.env.DB_POSTGRES_URL!);
    }

    private ensureTable(): void {
        if (!this.initialized) {
            const db = this.getDb();
            db.execute(
                "CREATE TABLE IF NOT EXISTS items (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
                []
            );
            this.initialized = true;
        }
    }

    @endpoint({ post: "/add" })
    async addItem(name: string): Promise<string> {
        this.ensureTable();
        const db = this.getDb();
        db.execute(
            "INSERT INTO items (name) VALUES ($1)",
            [{ tag: 'text', val: name } as DbValue]
        );
        return `Added: ${name}`;
    }

    @endpoint({ get: "/list" })
    async listItems(): Promise<string[]> {
        this.ensureTable();
        const db = this.getDb();
        const result: DbResult = db.query("SELECT name FROM items", []);
        return result.rows.map(row => {
            const val = row.values[0];
            return val.tag === 'text' ? val.val : String(val.val);
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

## Step 5: Configure Environment Variables

In `golem.yaml`, add the database URL under the component's `env` section:

```yaml
components:
  <app-name>:ts-main:
    env:
      DB_POSTGRES_URL: "postgres://golem:golem@localhost:5432/golem_test"
```

The component name follows the pattern `<app-name>:ts-main` for single-component TypeScript apps.

## Step 6: Build and Deploy

```shell
cd <APP_NAME>
golem build
golem deploy --yes
```

After deployment, HTTP endpoints are available at `http://<app-name>.localhost:9006/<mount-path>/<endpoint-path>`.

## Checklist

1. `golem new` executed successfully
2. Agent imports `DbConnection` from `'golem:rdbms/postgres@X.Y.Z'` (version from Step 2)
3. Agent uses `@agent({ mount: "..." })` and `@endpoint({ get/post: "..." })`
4. `DB_POSTGRES_URL` set in `golem.yaml` under component env
5. `golem build` succeeds
6. `golem deploy --yes` succeeds
