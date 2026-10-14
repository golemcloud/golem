import { z } from 'zod';
import {
  defineAgentClient,
  method,
  permissions,
  s,
  type PermissionCard,
  type ScopeCard,
} from '../src';

declare const card: PermissionCard;
declare const scope: ScopeCard;

const Worker = defineAgentClient({
  name: 'Worker',
  id: {},
  methods: {
    take: method({ input: { card: s.permissionCard({ polymorphic: false }) }, returns: z.void() }),
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
