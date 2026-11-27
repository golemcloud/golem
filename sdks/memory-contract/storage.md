# Storage

This page states what the database access tasks, the engine qualification tasks and the Standard
tools packages must preserve. Standard tools owns the engines and commands. Memory owns the
workloads, the fixtures and the SDK clients.

## Values

Every value is a schema-native typed value. The fixtures write each leaf in the shared Golem schema
encoding: the REST wire form that `ExternalSchemaValue` in `golem-common/src/schema/external.rs` reads
and writes (see `test-data/rest-wire-form/`). There is no parallel hand-written codec.

- **SQLite cells** keep null, signed 64-bit integers, reals, text and bytes. JavaScript uses `bigint`
  for every integer. `values-v1.json` fixes integers beyond the JavaScript safe range, the 64-bit
  bounds, whole-number reals that stay reals, -0.0, infinities, NaN (which SQLite stores as NULL),
  text that looks numeric, and empty and nonempty blobs. A REAL-affinity column loses the sign of
  -0.0, so a Memory schema that needs it uses an untyped column.
- **Cozo values** keep the integer and float distinction, bytes, lists, UUIDs, validity, JSON, and
  vectors with their precision and dimension. A vector is not a list of floats, and a UUID is not a
  string. The shared tool schema of the `cozo` tool carries these distinctions and serializes them
  with the shared Golem encoding. ST-32 defines that schema; until then each case marks its schema type
  as open. These requirements do not show a gap in the generic encoding.

A codec rejects internal or unsupported values. It never coerces through ordinary JSON numbers.

## Transactions

- SQLite access supplies `run(sql, params)` and `atomicBatch(statements)`. Cozo supplies one
  parameterized script call; the script is one transaction.
- One batch is one tool invocation and one transaction. A failed statement leaves none of the batch's
  writes (`batch/middle-statement-fails`). Separate `BEGIN`, write and `COMMIT` invocations cannot
  supply this guarantee.
- A Memory stores a source in one short transaction, ends it before model calls, then checks support
  and writes derived rows in one batch. Forgetting removes source, support and index rows in another
  batch.

## SQLite profile

`storage-v1.json` (`sqliteProfile`) fixes the profile of Memory-owned SQLite databases:

- Every open and reopen sets `PRAGMA temp_store = MEMORY` and `PRAGMA secure_delete = ON`.
- The durable initialization path selects and verifies `journal_mode = DELETE` and enables FTS5
  `secure-delete` on Memory's FTS tables. A read-only query does not repeat these persistent writes.
  A read-only open verifies the journal mode and does not convert the database.
- Every reader of a secure-delete FTS5 table, including external inspection programs, needs SQLite
  3.42.0 or later.
- The profile changes neither unrelated application databases nor Cozo's internal storage.

Direct APIs must work without each caller setting the temporary-storage pragma; the runtime owners
supply a central profile.

## Workloads

The SQLite workloads cover FTS5 matching, FTS5 secure deletion, a bounded recursive CTE over a cycle,
grouping and sorting that need a temporary b-tree, keyset pages and cosine similarity computed in SDK
code over bounded pages. The Cozo workloads cover a bounded recursive query, a bounded vector scan and
freshness after a durable write. The self-check runs the SQLite workloads on `node:sqlite`; ME-03 and
ST-31 run the Cozo workloads.

## Query lifecycle

An enforced query is an uncached, idempotent invocation that the engine prevents from mutating. The
`lifecycle` scenarios define the acceptance:

- The query protects the supplied database files and their sidecars through open, prepare, execute,
  finalize and close. Invocation-private scratch is allowed. A final unchanged hash alone is not proof.
- The engine refuses every mutating form with a structured error. `sqliteMutatingForms` and
  `cozoWorkloads.mutatingForms` list the forms; the self-check confirms that each SQLite form really
  writes a writable database. Text matching in Memory, or a transaction that never commits, is not
  enforcement.
- A query that needs recovery that writes protected files fails with the reason. It does not reopen
  the database writable, delete a journal or WAL file, or open with `immutable=1`.
- A query after a durable write returns the fresh result.
- A failed open leaves no file.

## Hooks

Memory-owned snapshot hooks use stable names that are unique and do not depend on the semantic tool
namespace. The `hooks` scenarios define cross-build restore, empty state for changed and added names,
duplicate-name rejection, unknown parts, genuine 1.6 TypeScript and Effect envelopes, refusal while a
transaction is open, and lazy reacquisition with the profile reapplied. Hook registration syntax and
the supply of stable names remain open.

## Package cards

A `sqlite3` or `cozo` tool declaration that omits its card inherits the package `initialCard`:
read, list, stat, write and delete on `filesystem(?env)` over `/**`, without a mount. A narrowed card
replaces it. Installation grants nothing. The `cards` scenarios define the checks. ST-28 and ST-32
supply the actual cards.
