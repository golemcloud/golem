import { describe, expect, it, vi } from "@effect/vitest"
import { inspect as nodeInspect } from "node:util"
import {
  Deferred,
  Effect,
  Equal,
  Exit,
  Fiber,
  Formatter,
  HashSet,
  Layer,
  Logger,
  Option,
  Schema,
} from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type * as TypesHost from "golem:permissions/types@0.1.0"
import * as AgentIdentity from "../src/AgentIdentity.js"
import { defineAgent } from "../src/Agent.js"
import * as Bridge from "../src/Bridge.js"
import * as Capability from "../src/Capability.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { PermissionsClient, type PermissionsClientShape } from "../src/host/PermissionsClient.js"
import { RpcClient, type RpcConnection } from "../src/host/RpcClient.js"
import * as Package from "../src/index.js"
import { method } from "../src/Method.js"
import * as Permissions from "../src/Permissions.js"
import * as DynamicClient from "../src/DynamicClient.js"
import { awaitInvocation } from "../src/internal/rpc.js"
import {
  GuestPermissionCardHandle,
  isGuestPermissionCardHandle,
} from "../src/internal/schema-model/permissionCardHandle.js"
import {
  createGuestQuotaTokenHandle,
  peekGuestQuotaTokenHandle,
} from "../src/internal/schema-model/quotaTokenHandle.js"
import { QUOTA_INTERNAL } from "../src/internal/schema-model/quotaInternal.js"
import {
  createGuestSecretHandle,
  peekGuestSecretHandle,
} from "../src/internal/schema-model/secretHandle.js"
import { SECRET_INTERNAL } from "../src/internal/schema-model/secretInternal.js"
import {
  restoreEncodedCapabilities,
  restoreEncodedCapabilitiesOnAbort,
  schemaValueToWitAsync,
} from "../src/internal/schema-model/wit.js"
import { schemaValueMatches } from "../src/internal/reflection/schemaValidation.js"
import { t, v } from "../src/internal/schema-model/model.js"
import { compile } from "../src/WitCodec.js"
import * as WitTypes from "../src/WitTypes.js"

interface FakeCard {
  readonly serial: number
  readonly parents: number[]
  readonly view: TypesHost.CardView
  readonly expiresAt?: TypesHost.Timestamp
}

const grant = (verb: string): Permissions.PatternGrant => ({
  class: "kv",
  owner: "?env",
  recipient: "*",
  verb,
  resourceId: "store.key",
})
const emptyView = (): TypesHost.CardView => ({
  lowerPositive: [],
  lowerNegative: [],
  upperPositive: [],
  upperNegative: [],
})
const cardId = (serial: number): TypesHost.CardId => ({
  uuid: { highBits: BigInt(serial), lowBits: 7n },
})

type WalletKind = TypesHost.WalletCard["tag"]

const makeHost = (walletKinds: ReadonlyArray<WalletKind> = ["concrete", "template"]) => {
  const events: string[] = []
  const failures = new Map<string, unknown>()
  let serial = 0
  const mint = (
    view: TypesHost.CardView,
    parents: number[] = [],
    expiresAt?: TypesHost.Timestamp,
  ) => {
    const card = { serial: ++serial, parents, view, expiresAt } as FakeCard
    Object.defineProperty(card, Symbol.dispose, {
      value: () => events.push(`dispose:${card.serial}`),
    })
    return card as unknown as CoreTypes.PermissionCard
  }
  const fake = (card: CoreTypes.PermissionCard) => card as unknown as FakeCard
  const fail = (operation: string) => {
    const failure = failures.get(operation)
    if (failure !== undefined) throw failure
  }
  const walletCards = walletKinds.map((kind) => ({ kind, card: fake(mint(emptyView())) }))
  const shape: PermissionsClientShape = {
    id: (card) => cardId(fake(card).serial),
    parents: (card) => fake(card).parents.map(cardId),
    expiresAt: (card) => fake(card).expiresAt,
    inspect: (card) => {
      fail("inspect")
      events.push(`inspect:${fake(card).serial}`)
      return fake(card).view
    },
    derive: (parent, lowerPositive, lowerNegative, upperPositive, upperNegative, lifespan) => {
      fail("derive")
      events.push(`derive:${fake(parent).serial}:${lifespan?.iso8601 ?? "-"}`)
      // A lifespan gives the child a fixed fake expiry that names its serial.
      return mint(
        { lowerPositive, lowerNegative, upperPositive, upperNegative },
        [fake(parent).serial],
        lifespan === undefined
          ? undefined
          : { seconds: 1_000n + BigInt(serial + 1), nanoseconds: 5 },
      )
    },
    deriveFromWallet: (lowerPositive, lowerNegative, upperPositive, upperNegative, lifespan) => {
      fail("deriveFromWallet")
      events.push(`deriveFromWallet:${lifespan?.iso8601 ?? "-"}`)
      return mint({ lowerPositive, lowerNegative, upperPositive, upperNegative })
    },
    deriveScope: (parents, lowerPositive, lowerNegative, upperPositive, upperNegative) => {
      events.push(`deriveScope:${parents.map((p) => fake(p).serial).join(",")}`)
      // Like the host, a scope card expires with the earliest of its parents.
      const expiries = parents.flatMap((p) =>
        fake(p).expiresAt === undefined ? [] : [fake(p).expiresAt!],
      )
      const earliest = expiries.reduce<TypesHost.Timestamp | undefined>(
        (min, next) => (min === undefined || next.seconds < min.seconds ? next : min),
        undefined,
      )
      return mint(
        { lowerPositive, lowerNegative, upperPositive, upperNegative },
        parents.map((p) => fake(p).serial),
        earliest,
      )
    },
    revoke: (card) => {
      events.push(`revoke:${fake(card).serial}`)
      fail("revoke")
      return 3
    },
    wallet: () =>
      walletCards.map(({ kind, card }) => ({
        tag: kind,
        val: card as unknown as CoreTypes.PermissionCard,
      })),
    walletVersion: () => ({ walletIdHash: new Uint8Array([1, 2]), generation: 5n }),
    install: (card, target) => {
      events.push(`install:${fake(card).serial}:${target.tag}`)
      fail("install")
    },
    listModules: () => [{ className: "kv", verbs: ["read", "write"] }],
    validateGrant: (g) => {
      events.push(`validate:${g.class_}:${g.verb}`)
      fail("validateGrant")
    },
    drop: (card) => {
      events.push(`drop:${fake(card).serial}`)
    },
  }
  return {
    events,
    failures,
    layer: Layer.succeed(PermissionsClient, PermissionsClient.of(shape)),
    serialOf: (card: CoreTypes.PermissionCard | undefined) =>
      card === undefined ? undefined : fake(card).serial,
  }
}

// The card of a wallet entry of the expected kind.
const cardOf = <T extends Permissions.WalletCard["_tag"]>(
  entry: Permissions.WalletCard | undefined,
  tag: T,
): Extract<Permissions.WalletCard, { readonly _tag: T }>["card"] => {
  if (entry?._tag !== tag) throw new Error(`expected a ${tag} wallet card, got ${entry?._tag}`)
  return entry.card as never
}

const failureOf = <A, E>(exit: Exit.Exit<A, E>): E => {
  if (Exit.isSuccess(exit)) throw new Error("expected a failure")
  const failure = exit.cause.reasons.find((reason) => reason._tag === "Fail")
  if (failure === undefined) throw new Error(`expected a typed failure: ${String(exit.cause)}`)
  return (failure as { readonly error: E }).error
}

describe("Permissions", () => {
  it.effect(
    "derives from the wallet, inspects, and derives a child that borrows its parent",
    () => {
      const host = makeHost()
      return Effect.gen(function* () {
        const card = yield* Permissions.deriveFromWallet(
          { lowerPositive: [grant("read")], upperPositive: [grant("read"), grant("write")] },
          Permissions.Lifespan.fromIso8601("P30D"),
        )
        const view = yield* Permissions.inspect(card)
        expect(view).toEqual({
          lowerPositive: [grant("read")],
          lowerNegative: [],
          upperPositive: [grant("read"), grant("write")],
          upperNegative: [],
        })
        const child = yield* Permissions.derive(
          card,
          { upperPositive: [grant("read")], upperNegative: [grant("write")] },
          Permissions.Lifespan.fromDuration("90 seconds"),
        )
        const childView = yield* Permissions.inspect(child)
        expect(childView.upperNegative).toEqual([grant("write")])
        const parentId = yield* Permissions.id(card)
        const childParents = yield* Permissions.parents(child)
        expect(childParents.length).toBe(1)
        expect(Equal.equals(childParents[0], parentId)).toBe(true)
        expect(Equal.equals(yield* Permissions.id(child), parentId)).toBe(false)
        expect(yield* Permissions.expiresAt(card)).toEqual(Option.none())
        yield* Permissions.inspect(card)
        expect(host.events).toEqual([
          "deriveFromWallet:P30D",
          "inspect:3",
          "derive:3:PT90S",
          "inspect:4",
          "inspect:3",
        ])
      }).pipe(Effect.provide(host.layer))
    },
  )

  it.effect("derives a scope card from several parents without consuming them", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const [concrete, template] = yield* Permissions.wallet
      const first = cardOf(concrete, "Concrete")
      const second = cardOf(template, "Template")
      const scope = yield* Permissions.deriveScope([first!, second!], {
        lowerPositive: [grant("read")],
      })
      const parents = yield* Permissions.parents(scope)
      const ids = HashSet.fromIterable([
        yield* Permissions.id(first!),
        yield* Permissions.id(second!),
      ])
      expect(parents.length).toBe(2)
      expect(parents.every((parent) => HashSet.has(ids, parent))).toBe(true)
      expect((yield* Permissions.inspect(scope)).lowerPositive).toEqual([grant("read")])
      expect(yield* Permissions.expiresAt(scope)).toEqual(Option.none())
      const nested = yield* Permissions.deriveScope([scope, first!], {})
      const nestedParents = yield* Permissions.parents(nested)
      expect(nestedParents.length).toBe(2)
      expect(Equal.equals(nestedParents[0], yield* Permissions.id(scope))).toBe(true)
      expect(Equal.equals(nestedParents[1], yield* Permissions.id(first!))).toBe(true)
      yield* Permissions.inspect(first!)
      yield* Permissions.inspect(second!)
      expect(host.events).toContain("deriveScope:3,1")
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("keeps scope cards out of persistent operations and schemas", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      const asCard = scope as unknown as Permissions.PermissionCard
      const target = Permissions.Holder.agent({
        componentId: { uuid: { highBits: 1n, lowBits: 2n } },
        agentId: "Target(1)",
      })
      for (const effect of [
        Permissions.revoke(asCard),
        Permissions.install(asCard, target),
        Permissions.derive(asCard, {}),
      ]) {
        expect(failureOf(yield* Effect.exit(effect))).toBeInstanceOf(
          Permissions.PermissionsHostError,
        )
      }
      expect(() => JSON.stringify(scope)).toThrow(/cannot be serialized/)
      expect(Schema.is(WitTypes.PermissionCard)(scope)).toBe(false)
      expect(() =>
        Bridge.permissionCardFromSchemaValue({ tag: "permission-card", handle: asCard }),
      ).toThrow(/does not carry a permission card/)
      yield* Effect.promise(() =>
        expect(schemaValueToWitAsync({ tag: "permission-card", handle: asCard })).rejects.toThrow(
          /invalid permission-card handle/,
        ),
      )

      yield* Permissions.release(scope)
      yield* Permissions.release(scope)
      expect(failureOf(yield* Effect.exit(Permissions.inspect(scope)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(host.events).toEqual(["deriveScope:1", "drop:3"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("moves the card before revoke, even when the host rejects the revocation", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      host.failures.set("revoke", { tag: "not-permitted", val: "card:revoke is not permitted" })
      const error = failureOf(yield* Effect.exit(Permissions.revoke(card)))
      expect(error).toBeInstanceOf(Permissions.PermissionError)
      expect(error).toMatchObject({ kind: "not-permitted", detail: "card:revoke is not permitted" })
      const reused = failureOf(yield* Effect.exit(Permissions.inspect(card)))
      expect(reused).toBeInstanceOf(Permissions.PermissionsHostError)
      expect(reused.message).toContain("moved")
      const again = failureOf(yield* Effect.exit(Permissions.revoke(card)))
      expect(again).toBeInstanceOf(Permissions.PermissionsHostError)
      expect(host.events.filter((event) => event.startsWith("revoke"))).toEqual(["revoke:3"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("returns the revoked count", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      expect(yield* Permissions.revoke(card)).toBe(3)
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("moves the card on install and reports a recipient mismatch", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const target = Permissions.Holder.agent({
        componentId: { uuid: { highBits: 1n, lowBits: 2n } },
        agentId: "Target(1)",
      })
      const card = yield* Permissions.deriveFromWallet({})
      host.failures.set("install", { tag: "install-recipient-mismatch", val: "wrong recipient" })
      const error = failureOf(yield* Effect.exit(Permissions.install(card, target)))
      expect(error).toMatchObject({ _tag: "PermissionError", kind: "install-recipient-mismatch" })
      expect(failureOf(yield* Effect.exit(Permissions.install(card, target)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      host.failures.delete("install")
      const next = yield* Permissions.deriveFromWallet({})
      yield* Permissions.install(next, target)
      expect(host.events.filter((event) => event.startsWith("install"))).toEqual([
        "install:3:agent",
        "install:4:agent",
      ])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("rejects an invalid account holder without moving the card", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const error = failureOf(
        yield* Effect.exit(Permissions.install(card, Permissions.Holder.account("not-a-uuid"))),
      )
      expect(error).toBeInstanceOf(Permissions.PermissionsHostError)
      yield* Permissions.inspect(card)
    }).pipe(Effect.provide(host.layer))
  })

  it.effect(
    "maps host-thrown permission errors through Error payloads and keeps other throws",
    () => {
      const host = makeHost()
      return Effect.gen(function* () {
        host.failures.set(
          "deriveFromWallet",
          Object.assign(new Error("host"), {
            payload: { tag: "wallet-multi-source-required", val: "two cards" },
          }),
        )
        expect(failureOf(yield* Effect.exit(Permissions.deriveFromWallet({})))).toMatchObject({
          _tag: "PermissionError",
          kind: "wallet-multi-source-required",
        })
        host.failures.set("deriveFromWallet", { tag: "card-polymorphic-not-usable", val: "x" })
        expect(failureOf(yield* Effect.exit(Permissions.deriveFromWallet({})))).toBeInstanceOf(
          Permissions.PermissionsHostError,
        )
        host.failures.set("validateGrant", { tag: "invalid-verb", val: "jump" })
        expect(
          failureOf(yield* Effect.exit(Permissions.validateGrant(grant("jump")))),
        ).toMatchObject({ kind: "invalid-verb", detail: "jump" })
      }).pipe(Effect.provide(host.layer))
    },
  )

  it("pins the permission-error kinds to the WIT variant", () => {
    expect(Permissions.permissionErrorKinds).toEqual([
      "not-permitted",
      "lower-bound-too-broad",
      "upper-bound-too-broad",
      "wallet-multi-source-required",
      "card-revoked",
      "card-expired",
      "install-recipient-mismatch",
      "unknown-resource-class",
      "invalid-pattern",
      "invalid-verb",
      "invalid-owner",
      "invalid-recipient",
    ])
  })

  it("builds lifespans from ISO-8601 strings and finite durations", () => {
    expect(Permissions.Lifespan.fromIso8601("P30D").iso8601).toBe("P30D")
    expect(() => Permissions.Lifespan.fromIso8601("30D")).toThrow(TypeError)
    expect(Permissions.Lifespan.fromDuration("1 hour").iso8601).toBe("PT3600S")
    expect(Permissions.Lifespan.fromDuration("1500 millis").iso8601).toBe("PT1.5S")
    expect(Permissions.Lifespan.fromDuration(5n).iso8601).toBe("PT0.000000005S")
    expect(() => Permissions.Lifespan.fromDuration("Infinity")).toThrow(TypeError)
    expect(() => Permissions.Lifespan.fromDuration(0)).toThrow(/positive/)
    expect(() => Permissions.Lifespan.fromDuration(-1500)).toThrow(/positive/)
    expect(() => Permissions.Lifespan.fromDuration(-1n)).toThrow(/positive/)
  })

  it.effect("reports the earliest root expiry of a scope card", () => {
    const host = makeHost(["concrete", "template"])
    return Effect.gen(function* () {
      const [concrete, template] = yield* Permissions.wallet
      const card = cardOf(concrete, "Concrete")
      const short = yield* Permissions.derive(card, {}, Permissions.Lifespan.fromIso8601("PT1H"))
      const long = yield* Permissions.derive(card, {}, Permissions.Lifespan.fromIso8601("P1D"))
      const shortExpiry = yield* Permissions.expiresAt(short)
      expect(shortExpiry).toEqual(Option.some({ seconds: 1_003n, nanoseconds: 5 }))
      const scope = yield* Permissions.deriveScope([long, cardOf(template, "Template"), short], {})
      expect(yield* Permissions.expiresAt(scope)).toEqual(shortExpiry)
      const nested = yield* Permissions.deriveScope([scope, card], {})
      expect(yield* Permissions.expiresAt(nested)).toEqual(shortExpiry)
      expect(yield* Permissions.expiresAt(yield* Permissions.deriveScope([card], {}))).toEqual(
        Option.none(),
      )
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("releases wrapped wallet cards and drops the rest when a wallet entry fails", () => {
    const host = makeHost(["concrete", "unknown" as WalletKind, "template"])
    return Effect.gen(function* () {
      const error = failureOf(yield* Effect.exit(Permissions.wallet))
      expect(error).toBeInstanceOf(Permissions.PermissionsHostError)
      expect(error.message).toContain("unknown wallet-card kind")
      expect(host.events).toEqual(["drop:1", "drop:2", "drop:3"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("releases wallet cards that are not kept", () => {
    const host = makeHost(["concrete", "template", "scope"])
    return Effect.gen(function* () {
      yield* Effect.scoped(
        Effect.gen(function* () {
          const [kept, revoked] = yield* Permissions.walletScoped
          yield* Permissions.revoke(cardOf(revoked, "Template"))
          yield* Permissions.release(cardOf(kept, "Concrete"))
          yield* Permissions.release(cardOf(kept, "Concrete"))
        }),
      )
      expect(host.events).toEqual(["revoke:2", "drop:1", "drop:3"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("reads the wallet version and kernel modules", () =>
    Effect.gen(function* () {
      expect(yield* Permissions.walletVersion).toEqual({
        walletIdHash: new Uint8Array([1, 2]),
        generation: 5n,
      })
      expect(yield* Permissions.listModules).toEqual([
        { className: "kv", verbs: ["read", "write"] },
      ])
    }).pipe(Effect.provide(makeHost().layer)),
  )

  it.effect("resolves an agent holder through the registered agent type", () =>
    Effect.gen(function* () {
      const identity = { typeName: "Reader", encoded: 'Reader("a")' } as AgentIdentity.Identity
      const holder = yield* Permissions.agentHolder(identity)
      expect(holder).toEqual({
        _tag: "Agent",
        agentId: { componentId: { uuid: { highBits: 9n, lowBits: 9n } }, agentId: 'Reader("a")' },
      })
      const missing = { typeName: "Missing", encoded: "Missing()" } as AgentIdentity.Identity
      expect(failureOf(yield* Effect.exit(Permissions.agentHolder(missing)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
    }).pipe(
      Effect.provideService(
        AgentHostClient,
        AgentHostClient.of({
          getAgentType: (name: string) =>
            name === "Reader"
              ? ({ implementedBy: { uuid: { highBits: 9n, lowBits: 9n } } } as never)
              : undefined,
        } as never),
      ),
    ),
  )
})

describe("Permissions non-forgeability", () => {
  it.effect("cards and card IDs cannot be constructed, serialized, or decoded from data", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      expect(() => JSON.stringify(card)).toThrow(/cannot be serialized/)
      expect(
        () =>
          new GuestPermissionCardHandle(Symbol("forged") as never, {} as CoreTypes.PermissionCard),
      ).toThrow(/internal SDK operation/)
      // `Capability` exports the card types as types only, so it has no runtime value to forge.
      expect("PermissionCard" in Capability).toBe(false)
      expect(Object.keys(Permissions)).not.toContain("PermissionCard")
      expect(Object.keys(Package.Permissions)).toEqual(Object.keys(Permissions))

      const id = yield* Permissions.id(card)
      expect(Object.keys(id)).toEqual([])
      expect(Object.getOwnPropertySymbols(id)).toEqual([])
      expect(() => new (Permissions.CardId as unknown as new () => unknown)()).toThrow(TypeError)

      expect(Schema.is(WitTypes.PermissionCard)({ serial: 3 })).toBe(false)
      const notACard = failureOf(
        yield* Effect.exit(
          Permissions.inspect({ serial: 3 } as unknown as Permissions.PermissionCard),
        ),
      )
      expect(notACard).toBeInstanceOf(Permissions.PermissionsHostError)
      expect(host.events).toEqual(["deriveFromWallet:-"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("card IDs print, log, and serialize only as a redacted placeholder", () => {
    const host = makeHost()
    const logged: Array<unknown> = []
    const logger = Logger.make<unknown, void>(({ message }) => {
      logged.push(message)
    })
    return Effect.gen(function* () {
      const id = yield* Permissions.id(yield* Permissions.deriveFromWallet({}))
      // The fake host's UUID for serial 3 renders as "3" and "7" in every hex or decimal form.
      const outputs = [
        String(id),
        `${id}`,
        JSON.stringify(id),
        JSON.stringify({ id }),
        nodeInspect(id),
        nodeInspect({ nested: [id] }, { depth: 5 }),
        Formatter.format(id),
        Formatter.format({ id }),
      ]
      yield* Effect.log(id, { id })
      outputs.push(JSON.stringify(logged))
      for (const output of outputs) {
        expect(output).toContain("CardId(<redacted>)")
        expect(output).not.toMatch(/[37]/)
      }
    }).pipe(Effect.provide(Layer.merge(host.layer, Logger.layer([logger]))))
  })

  it.effect("decodes only SDK-lifted cards from schema values", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      expect(Bridge.permissionCardFromSchemaValue(Bridge.permissionCardToSchemaValue(card))).toBe(
        card,
      )
      expect(() =>
        Bridge.permissionCardFromSchemaValue({
          tag: "permission-card",
          handle: { serial: 3 } as unknown as Permissions.PermissionCard,
        }),
      ).toThrow(/does not carry a permission card/)
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("shares the card registry with another copy of the SDK in the same component", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      vi.resetModules()
      const copy = yield* Effect.promise(
        () => import("../src/internal/schema-model/permissionCardHandle.js"),
      )
      expect(copy.isGuestPermissionCardHandle).not.toBe(isGuestPermissionCardHandle)
      expect(copy.isGuestPermissionCardHandle(card)).toBe(true)
    }).pipe(Effect.provide(host.layer))
  })
})

describe("wallet card kinds and polymorphic cards", () => {
  it.effect("labels every wallet card with its kind and type", () => {
    const host = makeHost(["concrete", "template", "scope"])
    return Effect.gen(function* () {
      const [concrete, template, scope] = yield* Permissions.wallet
      expect([concrete?._tag, template?._tag, scope?._tag]).toEqual([
        "Concrete",
        "Template",
        "Scope",
      ])
      expect(Schema.is(WitTypes.PermissionCard)(concrete!.card)).toBe(true)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(concrete!.card)).toBe(false)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(template!.card)).toBe(true)
      expect(Schema.is(WitTypes.PermissionCard)(template!.card)).toBe(false)
      expect(Schema.is(WitTypes.PermissionCard)(scope!.card)).toBe(false)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(scope!.card)).toBe(false)
      // The lent scope card is a scope card: it borrows, but cannot be revoked.
      yield* Permissions.inspect(cardOf(scope, "Scope"))
      expect(
        failureOf(
          yield* Effect.exit(
            Permissions.revoke(cardOf(scope, "Scope") as unknown as Permissions.PermissionCard),
          ),
        ),
      ).toBeInstanceOf(Permissions.PermissionsHostError)
      expect(host.events.filter((event) => event.startsWith("revoke"))).toEqual([])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("derives a concrete card from a template card and moves it on revoke", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      const child = yield* Permissions.derive(template, { lowerPositive: [grant("read")] })
      expect(Schema.is(WitTypes.PermissionCard)(child)).toBe(true)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(child)).toBe(false)
      expect(
        Equal.equals((yield* Permissions.parents(child))[0], yield* Permissions.id(template)),
      ).toBe(true)
      host.failures.set("revoke", { tag: "not-permitted", val: "card:revoke is not permitted" })
      expect(failureOf(yield* Effect.exit(Permissions.revoke(template)))).toMatchObject({
        kind: "not-permitted",
      })
      expect(failureOf(yield* Effect.exit(Permissions.inspect(template)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(failureOf(yield* Effect.exit(Permissions.revoke(template)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(host.events).toEqual(["derive:1:-", "revoke:1"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("moves a template card on install, even when the host rejects it", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      host.failures.set("install", { tag: "install-recipient-mismatch", val: "wrong recipient" })
      const target = Permissions.Holder.agent({
        componentId: { uuid: { highBits: 1n, lowBits: 2n } },
        agentId: "Target(1)",
      })
      expect(failureOf(yield* Effect.exit(Permissions.install(template, target)))).toMatchObject({
        kind: "install-recipient-mismatch",
      })
      expect(failureOf(yield* Effect.exit(Permissions.install(template, target)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(host.events).toEqual(["install:1:agent"])
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("keeps template cards unforgeable and separate from concrete cards", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      const concrete = yield* Permissions.deriveFromWallet({})
      expect(() => JSON.stringify(template)).toThrow(/cannot be serialized/)
      expect(Object.keys(Permissions)).not.toContain("PolymorphicPermissionCard")
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)({ serial: 1 })).toBe(false)
      expect(
        failureOf(
          yield* Effect.exit(
            Permissions.inspect({ serial: 1 } as unknown as Permissions.PolymorphicPermissionCard),
          ),
        ),
      ).toBeInstanceOf(Permissions.PermissionsHostError)

      const value = Bridge.polymorphicPermissionCardToSchemaValue(template)
      expect(Bridge.polymorphicPermissionCardFromSchemaValue(value)).toBe(template)
      expect(() => Bridge.permissionCardFromSchemaValue(value)).toThrow(
        /polymorphic permission card is not a concrete/,
      )
      expect(() =>
        Bridge.polymorphicPermissionCardFromSchemaValue(
          Bridge.permissionCardToSchemaValue(concrete),
        ),
      ).toThrow(/concrete permission card is not a polymorphic/)
      expect(() =>
        Bridge.polymorphicPermissionCardToSchemaValue(
          concrete as unknown as Permissions.PolymorphicPermissionCard,
        ),
      ).toThrow(/Expected a polymorphic permission card/)
      expect(() =>
        Bridge.permissionCardToSchemaValue(template as unknown as Permissions.PermissionCard),
      ).toThrow(/Expected a concrete permission card/)
      expect(() =>
        Bridge.polymorphicPermissionCardFromSchemaValue({
          tag: "permission-card",
          handle: { serial: 1 } as unknown as Permissions.PolymorphicPermissionCard,
        }),
      ).toThrow(/does not carry a permission card/)

      // Neither schema encodes a card of the other kind, and the card stays with its owner.
      const concreteCodec = yield* Effect.orDie(compile(WitTypes.PermissionCard))
      const templateCodec = yield* Effect.orDie(compile(WitTypes.PolymorphicPermissionCard))
      expect(
        Exit.isFailure(
          yield* Effect.exit(
            concreteCodec.encode(template as unknown as Permissions.PermissionCard),
          ),
        ),
      ).toBe(true)
      expect(
        Exit.isFailure(
          yield* Effect.exit(
            templateCodec.encode(concrete as unknown as Permissions.PolymorphicPermissionCard),
          ),
        ),
      ).toBe(true)
      yield* Permissions.inspect(template)
      yield* Permissions.inspect(concrete)
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("decodes a received card into the kind that the schema declares", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const templateCodec = yield* Effect.orDie(compile(WitTypes.PolymorphicPermissionCard))
      const concreteCodec = yield* Effect.orDie(compile(WitTypes.PermissionCard))
      const template = cardOf((yield* Permissions.wallet)[0], "Template")

      const sent = yield* Effect.orDie(templateCodec.encode(template))
      expect(failureOf(yield* Effect.exit(Permissions.inspect(template)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      const received = yield* Effect.orDie(templateCodec.decode(sent))
      expect(received).not.toBe(template)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(received)).toBe(true)
      yield* Permissions.inspect(received)

      // A decoded card takes the kind that the decoding schema declares, also after a local
      // encode. The host keeps the real kind, so this gives no authority.
      const resent = yield* Effect.orDie(templateCodec.encode(received))
      const relabelled = yield* Effect.orDie(concreteCodec.decode(resent))
      expect(Schema.is(WitTypes.PermissionCard)(relabelled)).toBe(true)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(relabelled)).toBe(false)

      // The host does not label a transferred card either.
      const fromHost = (serial: number): CoreTypes.SchemaValueTree => ({
        root: 0,
        valueNodes: [
          {
            tag: "permission-card-handle",
            val: { serial, parents: [], view: emptyView() } as unknown as CoreTypes.PermissionCard,
          },
        ],
      })
      const asConcrete = yield* Effect.orDie(concreteCodec.decode(fromHost(90)))
      expect(Schema.is(WitTypes.PermissionCard)(asConcrete)).toBe(true)
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(asConcrete)).toBe(false)
      const asTemplate = yield* Effect.orDie(templateCodec.decode(fromHost(91)))
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(asTemplate)).toBe(true)
      expect(templateCodec.graph.root.body).toEqual({
        tag: "permission-card",
        spec: { polymorphic: true },
      })
      expect(concreteCodec.graph.root.body).toEqual({
        tag: "permission-card",
        spec: { polymorphic: false },
      })
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("matches tool output cards against the declared kind", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      const concrete = yield* Permissions.deriveFromWallet({})
      const graph = (polymorphic: boolean) => ({
        root: t.permissionCard({ polymorphic }),
        defs: new Map(),
      })
      const matches = (polymorphic: boolean, card: GuestPermissionCardHandle) =>
        schemaValueMatches(graph(polymorphic), graph(polymorphic).root, v.permissionCard(card))
      expect(matches(true, template)).toBe(true)
      expect(matches(false, template)).toBe(false)
      expect(matches(false, concrete)).toBe(true)
      expect(matches(true, concrete)).toBe(false)
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("shares the kind of a card with another copy of the SDK", () => {
    const host = makeHost(["template"])
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      vi.resetModules()
      const copy = yield* Effect.promise(
        () => import("../src/internal/schema-model/permissionCardHandle.js"),
      )
      expect(copy.isGuestPermissionCardHandleOfKind(template, "template")).toBe(true)
      expect(copy.isGuestPermissionCardHandleOfKind(template, "concrete")).toBe(false)
    }).pipe(Effect.provide(host.layer))
  })
})

const Reader = defineAgent({
  name: "PermissionReader",
  id: { name: Schema.String },
  methods: {
    read: method({ input: {}, success: Schema.Void }),
    give: method({
      input: { card: WitTypes.PermissionCard },
      success: Schema.Void,
    }),
    giveTemplate: method({
      input: { card: WitTypes.PolymorphicPermissionCard },
      success: Schema.Void,
    }),
  },
})

const emptyRecord: Bridge.SchemaValue = { tag: "record", fields: [] }
const cardRecord = (card: Permissions.PermissionCard): Bridge.SchemaValue => ({
  tag: "record",
  fields: [Bridge.permissionCardToSchemaValue(card)],
})

interface RecordedCall {
  readonly kind: string
  readonly scopeCard: unknown
  readonly cards: ReadonlyArray<unknown>
}

const rpcLayer = (
  calls: Array<RecordedCall>,
  options: { readonly throwOnAwait?: boolean } = {},
) => {
  const metadata: AgentHost.InvocationMetadata = { agentId: "reader", idempotencyKey: "key" }
  const cardsOf = (input: CoreTypes.SchemaValueTree) =>
    input.valueNodes.flatMap((node) => (node.tag === "permission-card-handle" ? [node.val] : []))
  const connection: RpcConnection = {
    invokeAndAwait: () => {
      throw new Error("unused")
    },
    invoke: (_name, input) => {
      calls.push({ kind: "trigger", scopeCard: undefined, cards: cardsOf(input) })
      return metadata
    },
    asyncInvokeAndAwait: (_name, input, scopeCard) => {
      calls.push({ kind: "await", scopeCard, cards: cardsOf(input) })
      if (options.throwOnAwait) throw new Error("host rejected the call")
      return {
        metadata,
        get: () => Promise.resolve(undefined),
        cancel: () => {},
        drop: () => {},
      }
    },
    scheduleInvocation: () => {
      throw new Error("unused")
    },
    scheduleCancelableInvocation: () => {
      throw new Error("unused")
    },
    drop: () => {},
  }
  return Layer.mergeAll(
    Layer.succeed(RpcClient, RpcClient.of({ connect: () => Effect.succeed(connection) })),
    Layer.succeed(
      AgentHostClient,
      AgentHostClient.of({
        makeAgentId: () => "reader",
        parseAgentId: () => ["PermissionReader", { value: { root: 0, valueNodes: [] } }, undefined],
      } as never),
    ),
  )
}

describe("scope cards on awaited calls", () => {
  it.effect("attach a client default and a per-call scope card by borrowing them", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const defaultCard = yield* Permissions.deriveScope([walletCard], {})
      const perCall = yield* Permissions.deriveScope([walletCard], {})
      const plain = yield* Reader.client.get({ name: "a" })
      yield* plain.read({})
      const reader = yield* Reader.client.get({ name: "a" }, { scopeCard: defaultCard })
      yield* reader.read({})
      yield* reader.read.withScopeCard(perCall)({})
      yield* reader.read.trigger({})
      expect(calls.map((call) => [call.kind, host.serialOf(call.scopeCard as never)])).toEqual([
        ["await", undefined],
        ["await", 3],
        ["await", 4],
        ["trigger", undefined],
      ])
      yield* Permissions.inspect(defaultCard)
      yield* Permissions.inspect(perCall)

      yield* Permissions.release(perCall)
      const released = failureOf(yield* Effect.exit(reader.read.withScopeCard(perCall)({})))
      expect(released).toEqual({
        _tag: "ScopeCardUnavailableError",
        reason: "the scope card was released",
      })
      expect(calls.length).toBe(4)
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("check the scope card before an argument card is encoded", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      yield* Permissions.release(scope)
      const card = yield* Permissions.deriveFromWallet({})
      const reader = yield* Reader.client.get({ name: "a" })
      expect(
        failureOf(yield* Effect.exit(reader.give.withScopeCard(scope)({ card }))),
      ).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      const handle = yield* Bridge.resolveRemoteAgent(
        "PermissionReader",
        emptyRecord,
        undefined,
        [],
        "durable",
      )
      expect(
        failureOf(
          yield* Effect.exit(
            handle.invokeAndAwait("give", cardRecord(card), () => undefined, { scopeCard: scope }),
          ),
        ),
      ).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(calls).toEqual([])
      yield* Permissions.inspect(card)

      yield* reader.give({ card })
      expect(calls.map((call) => call.cards.map((raw) => host.serialOf(raw as never)))).toEqual([
        [4],
      ])
      expect(failureOf(yield* Effect.exit(Permissions.inspect(card)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("give encoded cards back when the scope card is released during encoding", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      const card = yield* Permissions.deriveFromWallet({})
      const connection = yield* (yield* RpcClient).connect(
        "PermissionReader",
        {} as never,
        undefined,
        [],
      )
      const encodeThenRelease = Effect.promise(() => schemaValueToWitAsync(cardRecord(card))).pipe(
        Effect.tap(() => Effect.orDie(Permissions.release(scope))),
      )
      const exit = yield* Effect.exit(awaitInvocation(connection, "give", encodeThenRelease, scope))
      expect(failureOf(exit)).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(calls).toEqual([])
      expect((yield* Permissions.inspect(card)).lowerPositive).toEqual([])
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("move a template card on send and give it back when the call fails", () => {
    const host = makeHost(["concrete", "template"])
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const [concrete, templateEntry] = yield* Permissions.wallet
      const template = cardOf(templateEntry, "Template")
      const scope = yield* Permissions.deriveScope([cardOf(concrete, "Concrete")], {})
      const connection = yield* (yield* RpcClient).connect(
        "PermissionReader",
        {} as never,
        undefined,
        [],
      )
      const encodeThenRelease = Effect.promise(() =>
        schemaValueToWitAsync({
          tag: "record",
          fields: [Bridge.polymorphicPermissionCardToSchemaValue(template)],
        }),
      ).pipe(Effect.tap(() => Effect.orDie(Permissions.release(scope))))
      const exit = yield* Effect.exit(
        awaitInvocation(connection, "giveTemplate", encodeThenRelease, scope),
      )
      expect(failureOf(exit)).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(calls).toEqual([])
      expect(Schema.is(WitTypes.PolymorphicPermissionCard)(template)).toBe(true)
      yield* Permissions.inspect(template)

      const reader = yield* Reader.client.get({ name: "a" })
      yield* reader.giveTemplate({ card: template })
      expect(calls.map((call) => call.cards.map((raw) => host.serialOf(raw as never)))).toEqual([
        [2],
      ])
      expect(failureOf(yield* Effect.exit(Permissions.inspect(template)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("do not give a template card back once the host call has started", () => {
    const host = makeHost(["template"])
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const template = cardOf((yield* Permissions.wallet)[0], "Template")
      const reader = yield* Reader.client.get({ name: "a" })
      expect(failureOf(yield* Effect.exit(reader.giveTemplate({ card: template })))).toMatchObject({
        _tag: "RpcCallError",
      })
      expect(calls.length).toBe(1)
      expect(failureOf(yield* Effect.exit(Permissions.inspect(template)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
    }).pipe(
      Effect.scoped,
      Effect.provide(Layer.merge(host.layer, rpcLayer(calls, { throwOnAwait: true }))),
    )
  })

  it.effect("attach per-call scope cards to bridge and dynamic awaited calls", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      const handle = yield* Bridge.resolveRemoteAgent(
        "PermissionReader",
        emptyRecord,
        undefined,
        [],
        "ephemeral",
      )
      yield* handle.invokeAndAwait("read", emptyRecord, () => undefined)
      yield* handle.invokeAndAwaitWithMetadata("read", emptyRecord, () => undefined, {
        scopeCard: scope,
      })
      yield* handle.invoke("read", emptyRecord)
      const identity = yield* AgentIdentity.parse('PermissionReader("a")')
      const dynamic = yield* DynamicClient.bind(identity)
      yield* dynamic.method("read").invoke({ root: 0, valueNodes: [] }, { scopeCard: scope })
      yield* dynamic.method("read").invoke({ root: 0, valueNodes: [] })
      expect(calls.map((call) => [call.kind, host.serialOf(call.scopeCard as never)])).toEqual([
        ["await", undefined],
        ["await", 3],
        ["trigger", undefined],
        ["await", 3],
        ["await", undefined],
      ])
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("drops a card that was released while it was on the wire", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      const card = yield* Permissions.deriveFromWallet({})
      const connection = yield* (yield* RpcClient).connect(
        "PermissionReader",
        {} as never,
        undefined,
        [],
      )
      const encodeThenRelease = Effect.promise(() => schemaValueToWitAsync(cardRecord(card))).pipe(
        Effect.tap(() => Effect.orDie(Permissions.release(card))),
        Effect.tap(() => Effect.orDie(Permissions.release(scope))),
      )
      const exit = yield* Effect.exit(awaitInvocation(connection, "give", encodeThenRelease, scope))
      expect(failureOf(exit)).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(failureOf(yield* Effect.exit(Permissions.inspect(card)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(host.events.filter((event) => /^(drop|dispose):4$/.test(event))).toEqual(["dispose:4"])
      expect(calls).toEqual([])
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("gives encoded cards back when the call is interrupted during encoding", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const connection = yield* (yield* RpcClient).connect(
        "PermissionReader",
        {} as never,
        undefined,
        [],
      )
      let openGate: () => void = () => {}
      const gate = new Promise<void>((resolve) => {
        openGate = resolve
      })
      const started = yield* Deferred.make<void>()
      // The encoder finishes only after the fiber is interrupted, like an encode whose promise
      // settles after the interruption aborted its signal.
      const encoding = Effect.promise((signal) => {
        Deferred.doneUnsafe(started, Exit.void)
        return restoreEncodedCapabilitiesOnAbort(
          signal,
          gate.then(() => schemaValueToWitAsync(cardRecord(card))),
        )
      })
      const fiber = yield* Effect.forkChild(awaitInvocation(connection, "give", encoding))
      yield* Deferred.await(started)
      yield* Fiber.interrupt(fiber)
      openGate()
      yield* Effect.promise(() => gate.then(() => new Promise((resolve) => setTimeout(resolve, 0))))
      expect(calls).toEqual([])
      expect((yield* Permissions.inspect(card)).lowerPositive).toEqual([])
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })

  it.effect("does not give cards back once the host call has started", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const reader = yield* Reader.client.get({ name: "a" })
      expect(failureOf(yield* Effect.exit(reader.give({ card })))).toMatchObject({
        _tag: "RpcCallError",
      })
      expect(calls.length).toBe(1)
      expect(failureOf(yield* Effect.exit(Permissions.inspect(card)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
    }).pipe(
      Effect.scoped,
      Effect.provide(Layer.merge(host.layer, rpcLayer(calls, { throwOnAwait: true }))),
    )
  })

  it.effect("restores secrets and quota tokens and drops streams of an unsent tree", () => {
    const host = makeHost()
    return Effect.gen(function* () {
      const card = yield* Permissions.deriveFromWallet({})
      const rawSecret = { secret: 1 } as never
      const rawToken = { token: 1 } as never
      const secret = createGuestSecretHandle(SECRET_INTERNAL, rawSecret)
      const token = createGuestQuotaTokenHandle(QUOTA_INTERNAL, rawToken)
      const tree = yield* Effect.promise(() =>
        schemaValueToWitAsync({
          tag: "record",
          fields: [
            Bridge.permissionCardToSchemaValue(card),
            { tag: "secret", handle: secret },
            { tag: "quota-token", handle: token },
          ],
        }),
      )
      expect(peekGuestSecretHandle(SECRET_INTERNAL, secret)).toBeUndefined()
      expect(peekGuestQuotaTokenHandle(QUOTA_INTERNAL, token)).toBeUndefined()
      restoreEncodedCapabilities(tree)
      expect(peekGuestSecretHandle(SECRET_INTERNAL, secret)).toBe(rawSecret)
      expect(peekGuestQuotaTokenHandle(QUOTA_INTERNAL, token)).toBe(rawToken)
      yield* Permissions.inspect(card)
      const carriers = tree.valueNodes.filter((node) =>
        ["permission-card-handle", "secret-value", "quota-token-handle"].includes(node.tag),
      )
      expect(carriers.map((node) => (node as { val: unknown }).val)).toEqual([
        undefined,
        undefined,
        undefined,
      ])

      const disposed: Array<string> = []
      const stream = { [Symbol.dispose]: () => disposed.push("stream") }
      const streamTree = {
        root: 0,
        valueNodes: [{ tag: "stream-value", val: stream }],
      } as unknown as CoreTypes.SchemaValueTree
      restoreEncodedCapabilities(streamTree)
      expect(disposed).toEqual(["stream"])
      expect((streamTree.valueNodes[0] as { val: unknown }).val).toBeUndefined()
    }).pipe(Effect.provide(host.layer))
  })

  it.effect("leaves a caller-supplied dynamic tree untouched when the call fails", () => {
    const host = makeHost()
    const calls: Array<RecordedCall> = []
    return Effect.gen(function* () {
      const walletCard = cardOf((yield* Permissions.wallet)[0], "Concrete")
      const scope = yield* Permissions.deriveScope([walletCard], {})
      const card = yield* Permissions.deriveFromWallet({})
      const tree = yield* Effect.promise(() => schemaValueToWitAsync(cardRecord(card)))
      const cardNode = tree.valueNodes.find((node) => node.tag === "permission-card-handle")!
      const raw = cardNode.val
      yield* Permissions.release(scope)
      const identity = yield* AgentIdentity.parse('PermissionReader("a")')
      const dynamic = yield* DynamicClient.bind(identity)
      expect(
        failureOf(yield* Effect.exit(dynamic.method("give").invoke(tree, { scopeCard: scope }))),
      ).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(cardNode.val).toBe(raw)

      // The same holds when the scope card is released after the tree was accepted.
      const later = yield* Permissions.deriveScope([walletCard], {})
      const connection = yield* (yield* RpcClient).connect(
        "PermissionReader",
        {} as never,
        undefined,
        [],
      )
      const supplied = Effect.succeed(tree).pipe(
        Effect.tap(() => Effect.orDie(Permissions.release(later))),
      )
      expect(
        failureOf(yield* Effect.exit(awaitInvocation(connection, "give", supplied, later, false))),
      ).toMatchObject({ _tag: "ScopeCardUnavailableError" })
      expect(cardNode.val).toBe(raw)
      expect(failureOf(yield* Effect.exit(Permissions.inspect(card)))).toBeInstanceOf(
        Permissions.PermissionsHostError,
      )
      expect(calls).toEqual([])
    }).pipe(Effect.scoped, Effect.provide(Layer.merge(host.layer, rpcLayer(calls))))
  })
})
