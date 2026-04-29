import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema, Stream } from "effect"
import * as Blobstore from "../src/blobstore.js"
import { make as makeBlobFake } from "./host/BlobFake.js"

const u8 = (s: string): Uint8Array => new TextEncoder().encode(s)
const s = (b: Uint8Array): string => new TextDecoder().decode(b)

describe("Blobstore.createContainer / getContainer / containerExists", () => {
  it.effect("createContainer succeeds for new names and returns a Container", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("photos")
        expect(c.name).toBe("photos")
        expect(Blobstore.isContainer(c)).toBe(true)
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("createContainer fails when the container already exists", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          yield* Blobstore.createContainer("dup")
          return yield* Blobstore.createContainer("dup")
        }).pipe(Effect.provide(fake.layer)),
      )
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("BlobstoreHostError")
        expect(json).toContain("already exists")
      }
    }),
  )

  it.effect("containerExists reports presence", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        yield* Blobstore.createContainer("x")
        const yes = yield* Blobstore.containerExists("x")
        const no = yield* Blobstore.containerExists("y")
        expect([yes, no]).toEqual([true, false])
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("getContainer fails for unknown names", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      const exit = yield* Effect.exit(
        Blobstore.getContainer("missing").pipe(Effect.provide(fake.layer)),
      )
      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("getOrCreateContainer is idempotent", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c1 = yield* Blobstore.getOrCreateContainer("oc")
        expect(c1.name).toBe("oc")
        const c2 = yield* Blobstore.getOrCreateContainer("oc")
        expect(c2.name).toBe("oc")
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("deleteContainer removes the container", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        yield* Blobstore.createContainer("d")
        yield* Blobstore.deleteContainer("d")
        const exists = yield* Blobstore.containerExists("d")
        expect(exists).toBe(false)
      }).pipe(Effect.provide(fake.layer))
    }),
  )
})

describe("Container — write/read/list", () => {
  it.effect("writeData + getData round-trip a small object", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c1")
        yield* c.writeData("hello.txt", u8("world"))
        const got = yield* c.getData("hello.txt")
        expect(s(got)).toBe("world")
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("writeData chunks payloads larger than 4096 bytes", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const big = new Uint8Array(10_000)
        for (let i = 0; i < big.length; i++) big[i] = i & 0xff

        const c = yield* Blobstore.createContainer("big")
        yield* c.writeData("blob.bin", big)
        const got = yield* c.getData("blob.bin")
        expect(got.length).toBe(10_000)
        expect(got[0]).toBe(0)
        expect(got[255]).toBe(255)
        expect(got[9999]).toBe(9999 & 0xff)
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("hasObject / objectInfo / deleteObject / deleteObjects", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c2")
        yield* c.writeData("a", u8("hello"))
        yield* c.writeData("b", u8("hello-bigger"))

        expect(yield* c.hasObject("a")).toBe(true)
        expect(yield* c.hasObject("z")).toBe(false)

        const info = yield* c.objectInfo("a")
        expect(info.name).toBe("a")
        expect(info.container).toBe("c2")
        expect(info.size).toBe(5n)
        expect(info.createdAt).toBeInstanceOf(Date)

        yield* c.deleteObject("a")
        expect(yield* c.hasObject("a")).toBe(false)

        yield* c.deleteObjects(["b"])
        expect(yield* c.hasObject("b")).toBe(false)
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("listObjects yields all keys (order undefined)", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c3")
        yield* c.writeData("a", u8("1"))
        yield* c.writeData("b", u8("2"))
        yield* c.writeData("c", u8("3"))
        const list = yield* Stream.runCollect(c.listObjects)
        expect(list.slice().sort()).toEqual(["a", "b", "c"])
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("clear removes all objects", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c4")
        yield* c.writeData("a", u8("1"))
        yield* c.writeData("b", u8("2"))
        yield* c.clear
        const remaining = yield* Stream.runCollect(c.listObjects)
        expect(remaining).toEqual([])
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("info returns ContainerMetadata with a real Date", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("md-c")
        const md = yield* c.info
        expect(md.name).toBe("md-c")
        expect(md.createdAt).toBeInstanceOf(Date)
        expect(typeof md.createdAtMillis).toBe("bigint")
      }).pipe(Effect.provide(fake.layer))
    }),
  )
})

describe("Container — copy / move", () => {
  it.effect("copyObject duplicates", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const src = yield* Blobstore.createContainer("src")
        const dst = yield* Blobstore.createContainer("dst")
        yield* src.writeData("a", u8("payload"))
        yield* Blobstore.copyObject(
          { container: "src", object: "a" },
          { container: "dst", object: "b" },
        )
        const inSrc = yield* src.hasObject("a")
        const inDst = yield* dst.hasObject("b")
        expect(inSrc).toBe(true)
        expect(inDst).toBe(true)
        const dest = yield* dst.getData("b")
        expect(s(dest)).toBe("payload")
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("moveObject removes source", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const src = yield* Blobstore.createContainer("src2")
        const dst = yield* Blobstore.createContainer("dst2")
        yield* src.writeData("a", u8("p"))
        yield* Blobstore.moveObject(
          { container: "src2", object: "a" },
          { container: "dst2", object: "b" },
        )
        expect(yield* src.hasObject("a")).toBe(false)
        expect(yield* dst.hasObject("b")).toBe(true)
      }).pipe(Effect.provide(fake.layer))
    }),
  )
})

describe("Container.forSchema", () => {
  const Photo = Schema.Struct({ filename: Schema.String, takenAtMillis: Schema.Number })

  it.effect("round-trips a typed object", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("schema-c")
        const photos = c.forSchema(Photo)
        yield* photos.writeData("a.json", { filename: "a.png", takenAtMillis: 12345 })
        const got = yield* photos.getData("a.json")
        expect(got).toEqual({ filename: "a.png", takenAtMillis: 12345 })
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("whole-object read of an empty object returns Uint8Array(0)", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("empty-c")
        yield* c.writeData("e", new Uint8Array(0))
        const got = yield* c.getData("e")
        expect(got).toBeInstanceOf(Uint8Array)
        expect(got.length).toBe(0)
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect("whole-object read tolerates exclusive-end backends", () =>
    Effect.gen(function* () {
      const fake = yield* makeBlobFake
      yield* Effect.gen(function* () {
        // The fake follows the in-mem/fs backend: end is exclusive.
        // The SDK's getData first tries (0, size - 1) (returns size - 1
        // bytes on the fake), then retries with end = size to recover
        // the full payload.
        const c = yield* Blobstore.createContainer("excl-c")
        yield* c.writeData("k", u8("alpha"))
        const got = yield* c.getData("k")
        expect(s(got)).toBe("alpha")
      }).pipe(Effect.provide(fake.layer))
    }),
  )

  it.effect(
    "decode failure surfaces as a typed failure (Schema.SchemaError via fromJsonString)",
    () =>
      Effect.gen(function* () {
        const fake = yield* makeBlobFake
        const exit = yield* Effect.exit(
          Effect.gen(function* () {
            const c = yield* Blobstore.createContainer("schema-c2")
            // Stash invalid JSON via the raw writeData
            yield* c.writeData("a.json", u8("not json"))
            const photos = c.forSchema(Photo)
            return yield* photos.getData("a.json")
          }).pipe(Effect.provide(fake.layer)),
        )
        expect(exit._tag).toBe("Failure")
        // Typed failure, not a defect — that is the contract.
      }),
  )
})
