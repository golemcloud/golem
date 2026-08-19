import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { defineAgent, Http, method } from "../src/index.js"
import { toWitCodec } from "../src/WitCodec.js"

enum UserConnectionType {
  Friend = "Friend",
  Follower = "Follower",
  Following = "Following",
}

enum LikeType {
  Like = "like",
  Insightful = "insightful",
  Love = "love",
  Dislike = "dislike",
}

enum Status {
  Pending = 0,
  Active = 1,
  Completed = 2,
}

enum HttpCode {
  Ok = 200,
  NotFound = 404,
}

describe("toWitCodec with Schema.Enum", () => {
  it.effect("round-trips string enum where keys equal values", () =>
    Effect.gen(function* () {
      const SchemaEnum = Schema.Enum(UserConnectionType)
      const wc = yield* toWitCodec(SchemaEnum)

      expect(wc.witType.nodes[0]?.type).toEqual({
        tag: "enum-type",
        val: ["Friend", "Follower", "Following"],
      })

      for (const val of [
        UserConnectionType.Friend,
        UserConnectionType.Follower,
        UserConnectionType.Following,
      ]) {
        const wv = yield* Schema.encodeEffect(wc.codec)(val)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect(back).toBe(val)
      }
    }),
  )

  it.effect("round-trips string enum where keys differ from values", () =>
    Effect.gen(function* () {
      const SchemaEnum = Schema.Enum(LikeType)
      const wc = yield* toWitCodec(SchemaEnum)

      expect(wc.witType.nodes[0]?.type).toEqual({
        tag: "enum-type",
        val: ["like", "insightful", "love", "dislike"],
      })

      for (const val of [LikeType.Like, LikeType.Insightful, LikeType.Love, LikeType.Dislike]) {
        const wv = yield* Schema.encodeEffect(wc.codec)(val)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect(back).toBe(val)
      }
    }),
  )

  it.effect("round-trips numeric enum", () =>
    Effect.gen(function* () {
      const SchemaEnum = Schema.Enum(Status)
      const wc = yield* toWitCodec(SchemaEnum)

      expect(wc.witType.nodes[0]?.type).toEqual({
        tag: "enum-type",
        val: ["Pending", "Active", "Completed"],
      })

      for (const val of [Status.Pending, Status.Active, Status.Completed]) {
        const wv = yield* Schema.encodeEffect(wc.codec)(val)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect(back).toBe(val)
      }
    }),
  )

  it.effect("round-trips non-sequential numeric enum", () =>
    Effect.gen(function* () {
      const SchemaEnum = Schema.Enum(HttpCode)
      const wc = yield* toWitCodec(SchemaEnum)

      expect(wc.witType.nodes[0]?.type).toEqual({
        tag: "enum-type",
        val: ["Ok", "NotFound"],
      })

      for (const val of [HttpCode.Ok, HttpCode.NotFound]) {
        const wv = yield* Schema.encodeEffect(wc.codec)(val)
        const back = yield* Schema.decodeEffect(wc.codec)(wv)
        expect(back).toBe(val)
      }
    }),
  )

  it.effect("round-trips Schema.Struct containing Schema.Enum", () =>
    Effect.gen(function* () {
      const Payload = Schema.Struct({
        conn: Schema.Enum(UserConnectionType),
        status: Schema.Enum(Status),
        message: Schema.String,
      })
      const wc = yield* toWitCodec(Payload)

      const val = {
        conn: UserConnectionType.Follower,
        status: Status.Active,
        message: "hello",
      }
      const wv = yield* Schema.encodeEffect(wc.codec)(val)
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toEqual(val)
    }),
  )

  it.effect("round-trips Schema.Array of Schema.Enum", () =>
    Effect.gen(function* () {
      const List = Schema.Array(Schema.Enum(LikeType))
      const wc = yield* toWitCodec(List)

      const val = [LikeType.Like, LikeType.Love, LikeType.Like]
      const wv = yield* Schema.encodeEffect(wc.codec)(val)
      const back = yield* Schema.decodeEffect(wc.codec)(wv)
      expect(back).toEqual(val)
    }),
  )

  it.effect("round-trips Schema.NullOr(Schema.Enum)", () =>
    Effect.gen(function* () {
      const Nullable = Schema.NullOr(Schema.Enum(UserConnectionType))
      const wc = yield* toWitCodec(Nullable)

      expect(wc.witType.nodes[0]?.type.tag).toBe("option-type")

      const val1 = UserConnectionType.Friend
      const wv1 = yield* Schema.encodeEffect(wc.codec)(val1)
      const back1 = yield* Schema.decodeEffect(wc.codec)(wv1)
      expect(back1).toBe(val1)

      const val2 = null
      const wv2 = yield* Schema.encodeEffect(wc.codec)(val2)
      const back2 = yield* Schema.decodeEffect(wc.codec)(wv2)
      expect(back2).toBeNull()
    }),
  )

  it.effect("round-trips Schema.Union containing Schema.Enum and primitive", () =>
    Effect.gen(function* () {
      const UnionSchema = Schema.Union([Schema.Enum(UserConnectionType), Schema.Number])
      const wc = yield* toWitCodec(UnionSchema)

      expect(wc.witType.nodes[0]?.type.tag).toBe("variant-type")

      const val1 = UserConnectionType.Following
      const wv1 = yield* Schema.encodeEffect(wc.codec)(val1)
      const back1 = yield* Schema.decodeEffect(wc.codec)(wv1)
      expect(back1).toBe(val1)

      const val2 = 42
      const wv2 = yield* Schema.encodeEffect(wc.codec)(val2)
      const back2 = yield* Schema.decodeEffect(wc.codec)(wv2)
      expect(back2).toBe(val2)
    }),
  )

  it("works in defineAgent with HTTP endpoint bindings", () => {
    const AgentDef = defineAgent({
      name: "EnumAgent",
      constructorParams: { name: Schema.String },
      http: Http.mount("/enum-agent/{name}"),
      methods: {
        getConnection: method({
          params: { type: Schema.Enum(UserConnectionType) },
          success: Schema.Enum(UserConnectionType),
          http: [Http.get("/connection?type={type}")],
        }),
        getStatus: method({
          params: { status: Schema.Enum(Status) },
          success: Schema.Enum(Status),
          http: [Http.get("/status/{status}")],
        }),
      },
    })

    expect(AgentDef.name).toBe("EnumAgent")
    expect(AgentDef.methods.getConnection).toBeDefined()
    expect(AgentDef.methods.getStatus).toBeDefined()
  })
})
