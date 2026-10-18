import { z } from 'zod';
import {
  defineAgentClient,
  method,
  permissions,
  s,
  type PermissionCard,
  type PolymorphicPermissionCard,
  type ScopeCard,
} from '../src';

declare const card: PermissionCard;
declare const template: PolymorphicPermissionCard;
declare const scope: ScopeCard;
declare const polymorphic: boolean;

const Worker = defineAgentClient({
  name: 'Worker',
  id: {},
  methods: {
    take: method({ input: { card: s.permissionCard({ polymorphic: false }) }, returns: z.void() }),
    takeTemplate: method({
      input: { card: s.permissionCard({ polymorphic: true }) },
      returns: s.permissionCard({ polymorphic: true }),
    }),
    run: method({ input: {}, returns: z.void() }),
  },
});

const client = Worker.client.get({}, undefined, { scopeCard: scope });
client.take({ card }, { scopeCard: scope });
client.run({ scopeCard: null });
permissions.deriveScope([card, scope], {});

// @ts-expect-error a persistent card is not a scope card
client.run({ scopeCard: card });
// @ts-expect-error the client default must be a scope card
Worker.client.get({}, undefined, { scopeCard: card });
// @ts-expect-error a scope card cannot be an RPC argument
client.take({ card: scope });
// @ts-expect-error a scope card cannot be revoked
scope.revoke();
// @ts-expect-error a scope card cannot be installed
scope.install(permissions.Holder.app(undefined as never));
// @ts-expect-error a scope card has no persistent derive
scope.derive({});
// @ts-expect-error a native lifespan cannot be calendar-based; it takes an SDK Duration
permissions.Lifespan.of('P1M');

client.takeTemplate({ card: template });
const derived: PermissionCard = template.derive({}).unwrap();
permissions.deriveScope([template, card, scope], {});
const expiry: permissions.Timestamp | undefined = scope.expiresAt();
void [derived, expiry];

async function returnedTemplate(): Promise<PolymorphicPermissionCard> {
  return client.takeTemplate({ card: template });
}
void returnedTemplate;

for (const entry of permissions.wallet()) {
  switch (entry.tag) {
    case 'concrete': {
      const concrete: PermissionCard = entry.card;
      // @ts-expect-error a concrete wallet card is not a template card
      const wrong: PolymorphicPermissionCard = entry.card;
      void [concrete, wrong];
      break;
    }
    case 'template': {
      const templateCard: PolymorphicPermissionCard = entry.card;
      // @ts-expect-error a template wallet card is not a concrete card
      const wrong: PermissionCard = entry.card;
      void [templateCard, wrong];
      break;
    }
    case 'scope': {
      const scopeCard: ScopeCard = entry.card;
      // @ts-expect-error a scope wallet card is not a persistent card
      const wrong: PermissionCard = entry.card;
      void [scopeCard, wrong];
      break;
    }
    default: {
      const exhaustive: never = entry;
      void exhaustive;
    }
  }
}

// @ts-expect-error a concrete card is not a template card
client.takeTemplate({ card });
// @ts-expect-error a template card is not a concrete card
client.take({ card: template });
// @ts-expect-error a template card is not a scope card
client.run({ scopeCard: template });
// @ts-expect-error the schema flag must be a literal, so the card type is known
s.permissionCard({ polymorphic });
// @ts-expect-error the wallet card kinds are closed
const unknownKind: permissions.WalletCard = { tag: 'other', card };
void unknownKind;

// An `any` flag matches the first overload, so the schema types as a concrete card whatever the
// flag is at runtime. Pass a literal `true` or `false`.
declare const anyFlag: any;
const AnyFlagWorker = defineAgentClient({
  name: 'AnyFlagWorker',
  id: {},
  methods: {
    take: method({
      input: { card: s.permissionCard({ polymorphic: anyFlag }) },
      returns: s.permissionCard({ polymorphic: anyFlag }),
    }),
  },
});
async function anyFlagTypesAsConcrete(): Promise<PermissionCard> {
  const anyFlagClient = AnyFlagWorker.client.get({});
  // @ts-expect-error the `any` flag types the parameter as a concrete card
  await anyFlagClient.take({ card: template });
  return anyFlagClient.take({ card });
}
void anyFlagTypesAsConcrete;
