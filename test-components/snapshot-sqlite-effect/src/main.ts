import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";
import { SqliteClient } from "@golemcloud/effect-golem/sqlite";
import { Effect, Schema } from "effect";
import { DatabaseSync } from "node:sqlite";

const schema = Schema.Struct({ count: Schema.Number });
const databases = ["memory", "temp", "file", "wrappedFile"] as const;
const methods = {
  append: method({ input: { value: Schema.Number }, success: Schema.Void }),
  inspect: method({ input: {}, success: Schema.String }),
};

const open = (name: string, count: number, restored: boolean) =>
  Effect.gen(function* () {
    const raw = yield* Effect.acquireRelease(
      Effect.sync(() =>
        [":memory:", "", `${name}.db`, `${name}-wrapped.db`].map(
          (path) => new DatabaseSync(path),
        ),
      ),
      (handles) => Effect.sync(() => handles.forEach((db) => db.close())),
    );
    const temp = yield* SqliteClient.fromDatabase(raw[1]!);
    const wrappedFile = yield* SqliteClient.fromDatabase(raw[3]!);
    if (!restored)
      for (const db of raw)
        db.exec("CREATE TABLE entries (value INTEGER NOT NULL)");
    return { count, raw, temp, wrappedFile };
  }).pipe(Effect.orDie);
type State = Effect.Success<ReturnType<typeof open>>;
const bindings = (s: State) => ({
  memory: s.raw[0]!,
  temp: s.temp,
  file: s.raw[2]!,
  wrappedFile: s.wrappedFile,
});
const handlers = (s: State) => ({
  append: ({ value }: { value: number }) =>
    Effect.gen(function* () {
      yield* Effect.sync(() => {
        s.raw[0]!.prepare("INSERT INTO entries VALUES (?)").run(value);
        s.raw[2]!.prepare("INSERT INTO entries VALUES (?)").run(value);
      });
      yield* s.temp`INSERT INTO entries VALUES (${value})`;
      yield* s.wrappedFile`INSERT INTO entries VALUES (${value})`;
      s.count++;
    }).pipe(Effect.orDie),
  inspect: () =>
    Effect.sync(() =>
      JSON.stringify({
        count: s.count,
        values: s.raw.map((db) =>
          db
            .prepare("SELECT value FROM entries ORDER BY rowid")
            .all()
            .map((row) => row.value),
        ),
      }),
    ),
});

defineAgent({
  name: "SimpleSqlite",
  id: { name: Schema.String },
  methods,
  snapshotting: Snapshot.define({
    schema,
    databases,
    policy: Snapshot.policy.everyN(2),
  }),
}).implement<State>({
  init: ({ name }) => open(name, 0, false),
  methods: handlers,
  snapshot: {
    save: (s) => Effect.succeed({ count: s.count }),
    restore: (saved, { id }) => open(id.name, saved.count, true),
    databases: bindings,
  },
});

defineAgent({
  name: "MultipartSqlite",
  id: { name: Schema.String },
  methods,
  snapshotting: Snapshot.multipart({
    schema,
    databases,
    policy: Snapshot.policy.everyN(2),
  }),
}).implement<State>({
  init: ({ name }) => open(name, 0, false),
  methods: handlers,
  snapshot: {
    save: (s) =>
      Effect.succeed({
        state: { count: s.count },
        parts: new Map([
          [
            "opaque",
            {
              bytes: Uint8Array.of(0, 255, 42),
              contentType: "application/octet-stream",
            },
          ],
        ]),
      }),
    restore: (saved, { id }) =>
      Effect.gen(function* () {
        const bytes = yield* Snapshot.requirePart(
          saved.parts,
          "opaque",
          "application/octet-stream",
        );
        if (bytes.join(",") !== "0,255,42")
          throw new Error("user part changed");
        return yield* open(id.name, saved.state.count, true);
      }),
    databases: bindings,
  },
});
