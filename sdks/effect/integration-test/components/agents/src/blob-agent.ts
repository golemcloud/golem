/**
 * BlobAgent — exercises the `effect-golem` `Blobstore` namespace
 * (`wasi:blobstore/*`) against the live Golem host.
 */
import { Effect, Schema, Stream } from "effect"
import { Blobstore, defineAgent, method } from "@golemcloud/effect-golem"

const Photo = Schema.Struct({ filename: Schema.String, takenAtMillis: Schema.Number })

export const BlobAgent = defineAgent({
  name: "BlobAgent",
  description: "Probe agent for wasi:blobstore container + object I/O",
  mode: "durable",
  id: { name: Schema.String },
  methods: {
    write: method({
      input: { key: Schema.String, value: Schema.String },
      success: Schema.Void,
    }),
    read: method({
      input: { key: Schema.String },
      success: Schema.String,
    }),
    has: method({
      input: { key: Schema.String },
      success: Schema.Boolean,
    }),
    size: method({
      input: { key: Schema.String },
      success: Schema.BigInt,
    }),
    list: method({
      input: {},
      success: Schema.Array(Schema.String),
    }),
    deleteOne: method({
      input: { key: Schema.String },
      success: Schema.Void,
    }),
    clear: method({
      input: {},
      success: Schema.Void,
    }),
    putPhoto: method({
      input: { key: Schema.String, filename: Schema.String, takenAtMillis: Schema.Number },
      success: Schema.Void,
    }),
    getPhoto: method({
      input: { key: Schema.String },
      success: Photo,
    }),
    writeBig: method({
      input: { key: Schema.String, len: Schema.Number },
      success: Schema.Void,
    }),
    readSize: method({
      input: { key: Schema.String },
      success: Schema.Number,
    }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    const container = yield* Blobstore.getOrCreateContainer(name)
    const photos = container.forSchema(Photo)
    const enc = (s: string) => new TextEncoder().encode(s)
    const dec = (b: Uint8Array) => new TextDecoder().decode(b)

    return {
      write: ({ key, value }) => container.writeData(key, enc(value)),
      read: ({ key }) => container.getData(key).pipe(Effect.map(dec)),
      has: ({ key }) => container.hasObject(key),
      size: ({ key }) => container.objectInfo(key).pipe(Effect.map((m) => m.size)),
      list: () => Stream.runCollect(container.listObjects).pipe(Effect.map((c) => c.slice())),
      deleteOne: ({ key }) => container.deleteObject(key),
      clear: () => container.clear,
      putPhoto: ({ key, filename, takenAtMillis }) =>
        photos.writeData(key, { filename, takenAtMillis }),
      getPhoto: ({ key }) => photos.getData(key),
      writeBig: ({ key, len }) =>
        Effect.gen(function* () {
          const buf = new Uint8Array(len)
          for (let i = 0; i < len; i++) buf[i] = i & 0xff
          yield* container.writeData(key, buf)
        }),
      readSize: ({ key }) => container.getData(key).pipe(Effect.map((b) => b.length)),
    }
  }),
)
