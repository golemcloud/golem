/**
 * KvAgent — exercises the `effect-golem` `KeyValue` namespace
 * (`wasi:keyvalue/eventual` + `eventual-batch`) against the live
 * Golem host.
 */
import { Effect, Option, Schema } from "effect"
import { defineAgent, KeyValue, method } from "effect-golem"

const User = Schema.Struct({ id: Schema.String, name: Schema.String })

export const KvAgent = defineAgent({
  name: "KvAgent",
  description: "Probe agent for wasi:keyvalue eventual + eventual-batch",
  mode: "durable",
  constructorParams: { name: Schema.String },
  methods: {
    putBytes: method({
      params: { key: Schema.String, value: Schema.String },
      success: Schema.Void,
    }),
    getBytes: method({
      params: { key: Schema.String },
      success: Schema.NullOr(Schema.String),
    }),
    exists: method({
      params: { key: Schema.String },
      success: Schema.Boolean,
    }),
    deleteKey: method({
      params: { key: Schema.String },
      success: Schema.Void,
    }),
    keys: method({
      params: {},
      success: Schema.Array(Schema.String),
    }),
    putUser: method({
      params: { id: Schema.String, name: Schema.String },
      success: Schema.Void,
    }),
    getUser: method({
      params: { id: Schema.String },
      success: Schema.NullOr(User),
    }),
    putBatch: method({
      params: {
        entries: Schema.Array(Schema.Tuple([Schema.String, Schema.String])),
      },
      success: Schema.Void,
    }),
    getBatch: method({
      params: { keys: Schema.Array(Schema.String) },
      success: Schema.Array(Schema.NullOr(Schema.String)),
    }),
  },
  impl: ({ name }) =>
    Effect.gen(function* () {
      const bucket = yield* KeyValue.openBucket(name)
      const users = bucket.forSchema(User)
      const enc = (s: string) => new TextEncoder().encode(s)
      const dec = (b: Uint8Array) => new TextDecoder().decode(b)

      return {
        putBytes: ({ key, value }) => bucket.set(key, enc(value)),
        getBytes: ({ key }) =>
          bucket.get(key).pipe(Effect.map((opt) => (Option.isSome(opt) ? dec(opt.value) : null))),
        exists: ({ key }) => bucket.exists(key),
        deleteKey: ({ key }) => bucket.delete(key),
        keys: () => bucket.keys.pipe(Effect.map((ks) => ks.slice())),
        putUser: ({ id, name }) => users.set(id, { id, name }),
        getUser: ({ id }) =>
          users.get(id).pipe(Effect.map((opt) => (Option.isSome(opt) ? opt.value : null))),
        putBatch: ({ entries }) => bucket.setMany(entries.map(([k, v]) => [k, enc(v)] as const)),
        getBatch: ({ keys }) =>
          bucket
            .getMany(keys)
            .pipe(
              Effect.map((arr) => arr.map((opt) => (Option.isSome(opt) ? dec(opt.value) : null))),
            ),
      }
    }),
})
