import { Duration, Effect, Equal, Option, Schema } from "effect";
import {
  defineAgent,
  defineAgentClient,
  method,
  Permissions,
} from "@golemcloud/effect-golem";

const Probe = Schema.Struct({
  walletKinds: Schema.Array(Schema.String),
  inspectAllowed: Schema.Boolean,
  scopeExpiresAt: Schema.NullOr(Schema.String),
});
type Probe = typeof Probe.Type;

const calleeMethods = {
  probe: method({ input: {}, success: Probe }),
  deriveWithoutAuthority: method({ input: {}, success: Schema.String }),
};
const calleeId = { name: Schema.String };

const Callee = defineAgent({
  name: "EffectPermissionCallee",
  id: calleeId,
  methods: calleeMethods,
});
const CalleeClient = defineAgentClient({
  name: "EffectPermissionCallee",
  id: calleeId,
  methods: calleeMethods,
});

const timestampText = (expiry: Option.Option<Permissions.Timestamp>): string | null =>
  Option.match(expiry, {
    onNone: () => null,
    onSome: ({ seconds, nanoseconds }) => `${seconds}.${nanoseconds}`,
  });

const errorTag = <A, R>(
  effect: Effect.Effect<A, Permissions.PermissionError | Permissions.PermissionsHostError, R>,
) =>
  effect.pipe(
    Effect.match({
      onFailure: (error) =>
        error._tag === "PermissionError" ? error.kind : error._tag,
      onSuccess: () => "ok",
    }),
  );

Callee.implement({
  init: () => Effect.void,
  methods: () => ({
    probe: () =>
      Effect.scoped(
        Effect.gen(function* () {
          const cards = yield* Permissions.walletScoped;
          let inspectAllowed = false;
          let scopeExpiresAt: string | null = null;
          for (const entry of cards) {
            const view = yield* Effect.option(Permissions.inspect(entry.card));
            if (Option.isSome(view)) inspectAllowed = true;
            if (entry._tag === "Scope") {
              scopeExpiresAt = timestampText(yield* Permissions.expiresAt(entry.card));
            }
          }
          return {
            // The wallet order is not part of the contract.
            walletKinds: cards.map((entry) => entry._tag).sort(),
            inspectAllowed,
            scopeExpiresAt,
          } satisfies Probe;
        }),
      ).pipe(Effect.orDie),
    deriveWithoutAuthority: () =>
      Effect.gen(function* () {
        const grant = {
          class: "card",
          owner: "*",
          recipient: "*",
          verb: "inspect",
          resourceId: "*",
        };
        return yield* errorTag(
          Permissions.deriveFromWallet({
            lowerPositive: [grant],
            upperPositive: [grant],
          }),
        );
      }),
  }),
});

const isInspectGrant = (grant: Permissions.PatternGrant) =>
  grant.class === "card" && grant.verb === "inspect" && grant.resourceId === "*";

const callErrorTag = <A, R>(effect: Effect.Effect<A, unknown, R>) =>
  effect.pipe(
    Effect.match({
      onFailure: (error) => {
        const { _tag, cause } = error as { _tag?: string; cause?: { tag?: string } };
        return cause?.tag === undefined ? String(_tag) : `${_tag}:${cause.tag}`;
      },
      onSuccess: () => "ok",
    }),
  );

// A scope card kept past the invocation that derived it.
let keptScope: Permissions.ScopeCard | undefined;

defineAgent({
  name: "EffectPermissionCaller",
  id: { name: Schema.String },
  methods: {
    run: method({ input: { callee: Schema.String }, success: Schema.String }),
    staleScopeCall: method({ input: { callee: Schema.String }, success: Schema.String }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    run: ({ callee: calleeName }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const result: Record<string, unknown> = {};

          const wallet = yield* Permissions.walletScoped;
          result.walletKinds = wallet.map((entry) => entry._tag).sort();
          let walletCard: Permissions.PersistentCard | undefined;
          let grant: Permissions.PatternGrant | undefined;
          for (const entry of wallet) {
            if (entry._tag === "Scope") continue;
            const view = yield* Effect.option(Permissions.inspect(entry.card));
            const found = Option.isSome(view)
              ? view.value.lowerPositive.find(isInspectGrant)
              : undefined;
            if (found !== undefined && walletCard === undefined) {
              walletCard = entry.card;
              grant = found;
            }
          }
          if (walletCard === undefined || grant === undefined)
            return yield* Effect.die("no wallet card grants card inspection");
          const bounds = { lowerPositive: [grant], upperPositive: [grant] };

          // 1. Derive from the wallet and 2. inspect.
          const card = yield* Permissions.deriveFromWallet(bounds);
          const view = yield* Permissions.inspect(card);
          result.derivedView = view;

          // 3. Derive children with lifespans, and derive from the template wallet card.
          const child = yield* Permissions.derive(
            card,
            bounds,
            Permissions.Lifespan.fromDuration(Duration.millis(3_600_250)),
          );
          const isoChild = yield* Permissions.derive(
            card,
            bounds,
            Permissions.Lifespan.fromIso8601("PT1H"),
          );
          const cardId = yield* Permissions.id(card);
          const childParents = yield* Permissions.parents(child);
          result.childParentMatches =
            childParents.length === 1 && Equal.equals(childParents[0], cardId);
          result.childExpires = Option.isSome(yield* Permissions.expiresAt(child));
          result.isoChildExpires = Option.isSome(yield* Permissions.expiresAt(isoChild));
          result.cardExpires = Option.isSome(yield* Permissions.expiresAt(card));
          result.templateDerive = yield* errorTag(
            Permissions.derive(walletCard, bounds),
          );

          // 4. Attach a scope card to awaited calls.
          const callee = yield* Callee.client.get({ name: calleeName });
          result.calleeBefore = yield* callee.probe({});
          const scope = yield* Permissions.deriveScope([walletCard], bounds);
          result.calleePerCallScope = yield* callee.probe.withScopeCard(scope)({});
          const scoped = yield* Callee.client.get(
            { name: calleeName },
            { scopeCard: scope },
          );
          result.calleeDefaultScope = yield* scoped.probe({});
          result.calleeAfterScope = yield* callee.probe({});
          result.scopeCardStillUsable =
            (yield* errorTag(Permissions.inspect(scope))) === "ok";
          result.scopeExpiresAt = timestampText(yield* Permissions.expiresAt(scope));

          keptScope = yield* Permissions.deriveScope([scope], bounds);

          // 5. Install a card on another agent.
          const identity = yield* CalleeClient.agentId({ name: calleeName });
          const holder = yield* Permissions.agentHolder(identity);
          const installed = yield* Permissions.derive(card, bounds);
          yield* Permissions.install(installed, holder);
          result.calleeAfterInstall = yield* callee.probe({});
          result.installedCardMoved = yield* errorTag(Permissions.inspect(installed));

          // 6. Revoke a card and its subtree, which includes the installed card.
          result.revokedCount = yield* Permissions.revoke(card);
          result.revokedCardReuse = yield* errorTag(Permissions.inspect(card));
          result.calleeAfterRevoke = yield* callee.probe({});

          // 7. Negative cases.
          const released = yield* Permissions.deriveScope([walletCard], bounds);
          yield* Permissions.release(released);
          result.releasedScopeCall = yield* callErrorTag(
            callee.probe.withScopeCard(released)({}),
          );
          const mismatched = yield* Permissions.deriveFromWallet({
            lowerPositive: [{ ...grant, recipient: "nobody@example.invalid/*/*/*/*" }],
            upperPositive: [{ ...grant, recipient: "nobody@example.invalid/*/*/*/*" }],
          });
          result.recipientMismatch = yield* errorTag(Permissions.install(mismatched, holder));
          result.accountHolder = yield* errorTag(
            Permissions.install(
              yield* Permissions.deriveFromWallet(bounds),
              Permissions.Holder.account("00000000-0000-0000-0000-000000000001"),
            ),
          );

          return JSON.stringify(result);
        }),
      ).pipe(Effect.orDie),
    staleScopeCall: ({ callee: calleeName }) =>
      Effect.scoped(
        Effect.gen(function* () {
          if (keptScope === undefined) return yield* Effect.die("no kept scope card");
          const callee = yield* Callee.client.get({ name: calleeName });
          return yield* callErrorTag(callee.probe.withScopeCard(keptScope)({}));
        }),
      ),
  }),
});
