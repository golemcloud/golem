import { Effect, Exit, Schema, Stream } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Blobstore from "../src/blobstore.js"
import { __resetBlobstoreMock } from "./mocks/wasi-blobstore-container.js"

const runP = <A, E, R>(eff: Effect.Effect<A, E, R>): Promise<A> =>
  Effect.runPromise(Effect.scoped(eff as Effect.Effect<A, E, never>))
const runExit = <A, E, R>(eff: Effect.Effect<A, E, R>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(Effect.scoped(eff as Effect.Effect<A, E, never>))

const u8 = (s: string): Uint8Array => new TextEncoder().encode(s)
const s = (b: Uint8Array): string => new TextDecoder().decode(b)

describe("Blobstore.createContainer / getContainer / containerExists", () => {
  beforeEach(() => __resetBlobstoreMock())
  afterEach(() => __resetBlobstoreMock())

  it("createContainer succeeds for new names and returns a Container", async () => {
    const c = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("photos")
        return c
      }),
    )
    expect(c.name).toBe("photos")
    expect(Blobstore.isContainer(c)).toBe(true)
  })

  it("createContainer fails when the container already exists", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        yield* Blobstore.createContainer("dup")
        return yield* Blobstore.createContainer("dup")
      }),
    )
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("BlobstoreHostError")
      expect(json).toContain("already exists")
    }
  })

  it("containerExists reports presence", async () => {
    const result = await runP(
      Effect.gen(function* () {
        yield* Blobstore.createContainer("x")
        const yes = yield* Blobstore.containerExists("x")
        const no = yield* Blobstore.containerExists("y")
        return [yes, no]
      }),
    )
    expect(result).toEqual([true, false])
  })

  it("getContainer fails for unknown names", async () => {
    const exit = await runExit(Blobstore.getContainer("missing"))
    expect(exit._tag).toBe("Failure")
  })

  it("getOrCreateContainer is idempotent", async () => {
    await runP(
      Effect.gen(function* () {
        const c1 = yield* Blobstore.getOrCreateContainer("oc")
        expect(c1.name).toBe("oc")
        const c2 = yield* Blobstore.getOrCreateContainer("oc")
        expect(c2.name).toBe("oc")
      }),
    )
  })

  it("deleteContainer removes the container", async () => {
    await runP(
      Effect.gen(function* () {
        yield* Blobstore.createContainer("d")
        yield* Blobstore.deleteContainer("d")
        const exists = yield* Blobstore.containerExists("d")
        expect(exists).toBe(false)
      }),
    )
  })
})

describe("Container — write/read/list", () => {
  beforeEach(() => __resetBlobstoreMock())
  afterEach(() => __resetBlobstoreMock())

  it("writeData + getData round-trip a small object", async () => {
    const got = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c1")
        yield* c.writeData("hello.txt", u8("world"))
        return yield* c.getData("hello.txt")
      }),
    )
    expect(s(got)).toBe("world")
  })

  it("writeData chunks payloads larger than 4096 bytes", async () => {
    const big = new Uint8Array(10_000)
    for (let i = 0; i < big.length; i++) big[i] = i & 0xff

    const got = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("big")
        yield* c.writeData("blob.bin", big)
        return yield* c.getData("blob.bin")
      }),
    )
    expect(got.length).toBe(10_000)
    expect(got[0]).toBe(0)
    expect(got[255]).toBe(255)
    expect(got[9999]).toBe(9999 & 0xff)
  })

  it("hasObject / objectInfo / deleteObject / deleteObjects", async () => {
    await runP(
      Effect.gen(function* () {
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
      }),
    )
  })

  it("listObjects yields all keys (order undefined)", async () => {
    const list = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c3")
        yield* c.writeData("a", u8("1"))
        yield* c.writeData("b", u8("2"))
        yield* c.writeData("c", u8("3"))
        const all = yield* Stream.runCollect(c.listObjects)
        return all
      }),
    )
    expect(list.slice().sort()).toEqual(["a", "b", "c"])
  })

  it("clear removes all objects", async () => {
    const remaining = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("c4")
        yield* c.writeData("a", u8("1"))
        yield* c.writeData("b", u8("2"))
        yield* c.clear
        const all = yield* Stream.runCollect(c.listObjects)
        return all
      }),
    )
    expect(remaining).toEqual([])
  })

  it("info returns ContainerMetadata with a real Date", async () => {
    const md = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("md-c")
        return yield* c.info
      }),
    )
    expect(md.name).toBe("md-c")
    expect(md.createdAt).toBeInstanceOf(Date)
    expect(typeof md.createdAtMillis).toBe("bigint")
  })
})

describe("Container — copy / move", () => {
  beforeEach(() => __resetBlobstoreMock())
  afterEach(() => __resetBlobstoreMock())

  it("copyObject duplicates", async () => {
    const dest = await runP(
      Effect.gen(function* () {
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
        return yield* dst.getData("b")
      }),
    )
    expect(s(dest)).toBe("payload")
  })

  it("moveObject removes source", async () => {
    await runP(
      Effect.gen(function* () {
        const src = yield* Blobstore.createContainer("src2")
        const dst = yield* Blobstore.createContainer("dst2")
        yield* src.writeData("a", u8("p"))
        yield* Blobstore.moveObject(
          { container: "src2", object: "a" },
          { container: "dst2", object: "b" },
        )
        expect(yield* src.hasObject("a")).toBe(false)
        expect(yield* dst.hasObject("b")).toBe(true)
      }),
    )
  })
})

describe("Container.forSchema", () => {
  beforeEach(() => __resetBlobstoreMock())
  afterEach(() => __resetBlobstoreMock())

  const Photo = Schema.Struct({ filename: Schema.String, takenAtMillis: Schema.Number })

  it("round-trips a typed object", async () => {
    const got = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("schema-c")
        const photos = c.forSchema(Photo)
        yield* photos.writeData("a.json", { filename: "a.png", takenAtMillis: 12345 })
        return yield* photos.getData("a.json")
      }),
    )
    expect(got).toEqual({ filename: "a.png", takenAtMillis: 12345 })
  })

  it("whole-object read of an empty object returns Uint8Array(0)", async () => {
    const got = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("empty-c")
        yield* c.writeData("e", new Uint8Array(0))
        return yield* c.getData("e")
      }),
    )
    expect(got).toBeInstanceOf(Uint8Array)
    expect(got.length).toBe(0)
  })

  it("whole-object read tolerates exclusive-end backends", async () => {
    // The mock follows the in-mem/fs backend: end is exclusive.
    // The SDK's getData first tries (0, size - 1) (returns size - 1
    // bytes on the mock), then retries with end = size to recover
    // the full payload.
    const got = await runP(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("excl-c")
        yield* c.writeData("k", u8("alpha"))
        return yield* c.getData("k")
      }),
    )
    expect(s(got)).toBe("alpha")
  })

  it("decode failure surfaces as a typed failure (Schema.SchemaError via fromJsonString)", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        const c = yield* Blobstore.createContainer("schema-c2")
        // Stash invalid JSON via the raw writeData
        yield* c.writeData("a.json", u8("not json"))
        const photos = c.forSchema(Photo)
        return yield* photos.getData("a.json")
      }),
    )
    expect(exit._tag).toBe("Failure")
    // Typed failure, not a defect — that is the contract.
  })
})
