---
name: golem-list-and-filter-agents
description: "Listing and querying agents with filters. Use when listing all agents, filtering agents by name, status, revision, creation time, or environment variables, or paginating through agent results."
---

# Listing and Filtering Agents

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## `agent list` — List All Agents

```shell
golem agent list
```

Lists all agents across all deployed components in the current application. The output includes each agent's name, component, status, and revision.

### Filtering by Agent Type

Pass an agent type name as a positional argument to list only agents of that type:

```shell
golem agent list CounterAgent
```

### Filtering by Component

Use `--component-name` to list agents belonging to a specific component:

```shell
golem agent list --component-name my-component
```

> **Note**: `--component-name` and the agent type name positional argument are mutually exclusive.

### Property-Based Filters (`--filter`)

Use `--filter` to filter agents by metadata properties. Each filter has the format `property comparator value`. Multiple `--filter` flags are combined with AND logic.

#### Filterable Properties

| Property | Comparators | Example |
|----------|------------|---------|
| `name` | `=`, `!=`, `like`, `notlike`, `startswith` | `--filter "name = CounterAgent(\"c1\")"` |
| `status` | `=`, `!=`, `>`, `>=`, `<`, `<=` | `--filter "status = Running"` |
| `revision` | `=`, `!=`, `>`, `>=`, `<`, `<=` | `--filter "revision >= 2"` |
| `created_at` | `=`, `!=`, `>`, `>=`, `<`, `<=` | `--filter "created_at > 2025-01-01T00:00:00Z"` |
| `env.<VAR>` | `=`, `!=`, `like`, `notlike`, `startswith` | `--filter "env.MODE = production"` |

#### Agent Status Values

Valid status values: `Running`, `Idle`, `Suspended`, `Interrupted`, `Retrying`, `Failed`, `Exited`.

#### String Comparators

| Comparator | Aliases | Description |
|-----------|---------|-------------|
| `=` | `==`, `equal`, `eq` | Exact match |
| `!=` | `notequal`, `ne` | Not equal |
| `like` | — | Contains substring |
| `notlike` | — | Does not contain substring |
| `startswith` | — | Starts with prefix |

#### Numeric/Ordinal Comparators (for `status`, `revision`, `created_at`)

| Comparator | Aliases | Description |
|-----------|---------|-------------|
| `=` | `==`, `equal`, `eq` | Equal |
| `!=` | `notequal`, `ne` | Not equal |
| `>` | `greater`, `gt` | Greater than |
| `>=` | `greaterequal`, `ge` | Greater than or equal |
| `<` | `less`, `lt` | Less than |
| `<=` | `lessequal`, `le` | Less than or equal |

### Combining Filters

Multiple `--filter` flags are combined with AND:

```shell
golem agent list --filter "status = Running" --filter "name like counter"
```

### Pagination

Use `--max-count` to limit the number of results and `--scan-cursor` to paginate:

```shell
golem agent list --max-count 10
golem agent list --max-count 10 --scan-cursor 0/5
```

The cursor is returned in the output when there are more results. Use it in the next call to get the next page.

> **Note**: `--scan-cursor` requires a single component to be selected (either via `--component-name` or by being in a single-component application directory).

### Precise Mode

Use `--precise` to query the most up-to-date status for each agent (slightly slower):

```shell
golem agent list --precise
```

### Watch Mode (`--refresh`)

Use `--refresh` to continuously refresh the agent list on screen:

```shell
golem agent list --refresh           # Default 400ms interval
golem agent list --refresh=1000      # Custom 1-second interval
```

> **Note**: `--refresh` conflicts with `--scan-cursor`.

## Examples

List all agents:
```shell
golem agent list
```

List only running agents:
```shell
golem agent list --filter "status = Running"
```

List agents of a specific type:
```shell
golem agent list CounterAgent
```

Find agents by name pattern:
```shell
golem agent list --filter "name like test"
```

List agents with a specific environment variable value:
```shell
golem agent list --filter "env.MODE = production"
```

Combine filters (AND logic):
```shell
golem agent list --filter "status = Idle" --filter "revision >= 2"
```
