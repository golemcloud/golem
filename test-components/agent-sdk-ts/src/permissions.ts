// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Agents exercising the typed permission-card API end to end. The executor test
// grants `PermissionCaller` the concrete grants below and gives
// `PermissionCallee` and `PermissionNoAuthority` no host permissions beyond the
// GOLEM_* environment variables.

import { z } from 'zod';
import {
  defineAgent,
  Duration,
  getSelfMetadata,
  isRemoteCallError,
  method,
  permissions,
  s,
  type PolymorphicPermissionCard,
} from '@golemcloud/golem-ts-sdk';
import { getEnvironment } from 'wasi:cli/environment@0.3.0';

const SCOPE_ENV = 'SCOPE_ENV';
const ABSENT = '<absent>';

const inspectGrant: permissions.PatternGrant = {
  class: 'card',
  owner: '*',
  recipient: '*',
  verb: 'inspect',
  resourceId: '*',
};

const envGrant: permissions.PatternGrant = {
  class: 'env',
  owner: '*/*/*/*/*',
  recipient: '*',
  verb: 'read',
  resourceId: SCOPE_ENV,
};

const mismatchedRecipientGrant: permissions.PatternGrant = {
  ...inspectGrant,
  recipient: 'nobody@example.com/*/*/*/*',
};

function render(grant: permissions.PatternGrant): string {
  return `${grant.class}(${grant.owner}) @ ${grant.recipient} : ${grant.verb} : ${grant.resourceId}`;
}

function sameGrant(left: permissions.PatternGrant, right: permissions.PatternGrant): boolean {
  return render(left) === render(right);
}

/** The template wallet card with the caller's env grant. Releases the other wallet cards. */
function templateCard(): PolymorphicPermissionCard {
  let template: PolymorphicPermissionCard | undefined;
  for (const entry of permissions.wallet()) {
    const view =
      template === undefined && entry.tag === 'template' ? entry.card.inspect() : undefined;
    if (
      entry.tag === 'template' &&
      view !== undefined &&
      view.isOk() &&
      view.unwrap().lowerPositive.some((grant) => sameGrant(grant, envGrant))
    ) {
      template = entry.card;
    } else {
      entry.card.release();
    }
  }
  if (template === undefined) {
    throw new Error('the caller wallet has no template card with the env grant');
  }
  return template;
}

/**
 * The sorted kinds of this agent's wallet cards, and the expiry of a listed
 * scope card. Releases every card.
 */
function walletKinds(): { kinds: string[]; scopeExpiresAt: number } {
  const kinds: string[] = [];
  let scopeExpiresAt = -1;
  for (const entry of permissions.wallet()) {
    kinds.push(entry.tag);
    if (entry.tag === 'scope') {
      const expiresAt = entry.card.expiresAt();
      scopeExpiresAt = expiresAt === undefined ? 0 : Number(expiresAt.seconds);
    }
    entry.card.release();
  }
  return { kinds: kinds.sort(), scopeExpiresAt };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const PermissionCallee = defineAgent({
  name: 'PermissionCallee',
  id: { name: z.string() },
  methods: {
    scopeEnv: method({ input: {}, returns: z.string() }),
    walletKinds: method({
      input: {},
      returns: z.object({ kinds: z.array(z.string()), scopeExpiresAt: z.number() }),
    }),
    echoCard: method({
      input: { card: s.permissionCard({ polymorphic: false }) },
      returns: s.permissionCard({ polymorphic: false }),
    }),
    echoTemplate: method({
      input: { card: s.permissionCard({ polymorphic: true }) },
      returns: s.permissionCard({ polymorphic: true }),
    }),
  },
});

export const PermissionCalleeImpl = PermissionCallee.implement({
  init: () => ({}),
  methods: {
    scopeEnv() {
      return getEnvironment().find(([key]) => key === SCOPE_ENV)?.[1] ?? ABSENT;
    },
    walletKinds() {
      return walletKinds();
    },
    echoCard({ card }) {
      if (card.isMoved()) throw new Error('the received card is not usable');
      return card;
    },
    echoTemplate({ card }) {
      if (card.isMoved()) throw new Error('the received template card is not usable');
      return card;
    },
  },
});

export const PermissionCaller = defineAgent({
  name: 'PermissionCaller',
  id: { name: z.string() },
  methods: {
    walletDerivation: method({
      input: {},
      returns: z.object({
        parentFromWallet: z.boolean(),
        parentGrants: z.array(z.string()),
        parentExpires: z.boolean(),
        childOfParent: z.boolean(),
        childGrants: z.array(z.string()),
        childLifespanSeconds: z.number(),
      }),
    }),
    scopeRpc: method({
      input: { calleeName: z.string() },
      returns: z.object({
        before: z.string(),
        during: z.string(),
        after: z.string(),
        clientDefault: z.string(),
        suppressed: z.string(),
        scopeCardKept: z.boolean(),
      }),
    }),
    installAndRevoke: method({
      input: { calleeName: z.string() },
      returns: z.object({
        walletBefore: z.array(z.string()),
        installed: z.boolean(),
        movedError: z.string(),
        walletAfterInstall: z.array(z.string()),
        envAfterInstall: z.string(),
        revokedCount: z.number(),
        walletAfterRevoke: z.array(z.string()),
        envAfterRevoke: z.string(),
      }),
    }),
    walletCardKinds: method({
      input: { calleeName: z.string() },
      returns: z.object({
        ownBefore: z.array(z.string()),
        own: z.array(z.string()),
        templateChildIsConcrete: z.boolean(),
        longExpiresAt: z.number(),
        parentExpiresAt: z.number(),
        scopeExpiresAt: z.number(),
        calleeWithoutScope: z.array(z.string()),
        calleeWithScope: z.array(z.string()),
        calleeScopeExpiresAt: z.number(),
      }),
    }),
    keepScopeCard: method({ input: {}, returns: z.void() }),
    useKeptScopeCard: method({ input: { calleeName: z.string() }, returns: z.string() }),
    cardRoundTrip: method({
      input: { calleeName: z.string() },
      returns: z.object({
        movedAfterSend: z.boolean(),
        sameCard: z.boolean(),
        returnedInspectable: z.boolean(),
        templateMovedAfterSend: z.boolean(),
        sameTemplate: z.boolean(),
        returnedTemplateInspectable: z.boolean(),
      }),
    }),
    recipientMismatch: method({
      input: { calleeName: z.string() },
      returns: z.object({ error: z.string(), moved: z.boolean() }),
    }),
  },
});

function calleeHolder(calleeName: string): permissions.Holder {
  return permissions.Holder.agent({
    componentId: getSelfMetadata().agentId.componentId,
    agentId: PermissionCallee.agentId({ name: calleeName }).value,
  });
}

export const PermissionCallerImpl = PermissionCaller.implement({
  init: (): { keptScope?: permissions.ScopeCard } => ({}),
  methods: {
    walletDerivation() {
      const template = templateCard();
      const parent = permissions
        .deriveFromWallet({ lowerPositive: [inspectGrant, envGrant] })
        .unwrap();
      const child = parent
        .derive({ lowerPositive: [envGrant] }, permissions.Lifespan.of(Duration.hours(1)))
        .unwrap();
      const now = BigInt(Math.floor(Date.now() / 1000));
      const expiresAt = child.expiresAt();
      const result = {
        parentFromWallet: parent.parents().includes(template.id()),
        parentGrants: parent.inspect().unwrap().lowerPositive.map(render),
        parentExpires: parent.expiresAt() !== undefined,
        childOfParent: child.parents().length === 1 && child.parents()[0] === parent.id(),
        childGrants: child.inspect().unwrap().lowerPositive.map(render),
        childLifespanSeconds: expiresAt === undefined ? -1 : Number(expiresAt.seconds - now),
      };
      template.release();
      parent.release();
      child.release();
      return result;
    },

    async scopeRpc({ calleeName }) {
      const template = templateCard();
      const scope = permissions.deriveScope([template], { lowerPositive: [envGrant] }).unwrap();
      const callee = PermissionCallee.client.get({ name: calleeName });
      const before = await callee.scopeEnv();
      const during = await callee.scopeEnv({ scopeCard: scope });
      const after = await callee.scopeEnv();
      const scoped = PermissionCallee.client.get({ name: calleeName }, undefined, {
        scopeCard: scope,
      });
      const clientDefault = await scoped.scopeEnv();
      const suppressed = await scoped.scopeEnv({ scopeCard: null });
      const scopeCardKept = !scope.isMoved() && !template.isMoved();
      scope.release();
      template.release();
      return { before, during, after, clientDefault, suppressed, scopeCardKept };
    },

    async installAndRevoke({ calleeName }) {
      const callee = PermissionCallee.client.get({ name: calleeName });
      const walletBefore = (await callee.walletKinds()).kinds;
      const parent = permissions
        .deriveFromWallet({ lowerPositive: [inspectGrant, envGrant] })
        .unwrap();
      const child = parent.derive({ lowerPositive: [envGrant] }).unwrap();
      const installed = child.install(calleeHolder(calleeName)).isOk();
      let movedError = '';
      try {
        child.id();
      } catch (error) {
        movedError = errorMessage(error);
      }
      const walletAfterInstall = (await callee.walletKinds()).kinds;
      const envAfterInstall = await callee.scopeEnv();
      const keptChild = parent.derive({ lowerPositive: [envGrant] }).unwrap();
      const revokedCount = parent.revoke().unwrap();
      const walletAfterRevoke = (await callee.walletKinds()).kinds;
      const envAfterRevoke = await callee.scopeEnv();
      keptChild.release();
      return {
        walletBefore,
        installed,
        movedError,
        walletAfterInstall,
        envAfterInstall,
        revokedCount,
        walletAfterRevoke,
        envAfterRevoke,
      };
    },

    async walletCardKinds({ calleeName }) {
      const ownBefore = walletKinds().kinds;
      const template = templateCard();
      const deriveFor = (hours: number) =>
        template
          .derive(
            { lowerPositive: [inspectGrant, envGrant] },
            permissions.Lifespan.of(Duration.hours(hours)),
          )
          .unwrap();
      const longParent = deriveFor(2);
      const parent = deriveFor(1);
      const templateChildIsConcrete = parent instanceof permissions.PermissionCard;
      const longExpiresAt = longParent.expiresAt()?.seconds ?? -1n;
      const parentExpiresAt = parent.expiresAt()?.seconds ?? -1n;
      template.release();
      // A persistent scope parent must be in the wallet, so install the expiring cards on this
      // agent and take them from the wallet again.
      const self = permissions.Holder.agent(getSelfMetadata().agentId);
      longParent.install(self).unwrap();
      parent.install(self).unwrap();
      const own: string[] = [];
      let walletTemplate: PolymorphicPermissionCard | undefined;
      let walletLong: permissions.PermissionCard | undefined;
      let walletShort: permissions.PermissionCard | undefined;
      for (const entry of permissions.wallet()) {
        own.push(entry.tag);
        const expiresAt = entry.tag === 'concrete' ? entry.card.expiresAt()?.seconds : undefined;
        if (entry.tag === 'template' && walletTemplate === undefined) {
          walletTemplate = entry.card;
        } else if (entry.tag === 'concrete' && expiresAt === longExpiresAt) {
          walletLong = entry.card;
        } else if (entry.tag === 'concrete' && expiresAt === parentExpiresAt) {
          walletShort = entry.card;
        } else {
          entry.card.release();
        }
      }
      if (walletTemplate === undefined || walletLong === undefined || walletShort === undefined) {
        throw new Error('the wallet lacks the template or an installed concrete card');
      }
      // The two-hour root comes first and the template root never expires, so only the earliest
      // root expiry gives the one-hour expiry of `parent`.
      const scope = permissions
        .deriveScope([walletLong, walletTemplate, walletShort], {
          lowerPositive: [inspectGrant, envGrant],
        })
        .unwrap();
      const callee = PermissionCallee.client.get({ name: calleeName });
      const calleeWithoutScope = await callee.walletKinds();
      const calleeWithScope = await callee.walletKinds({ scopeCard: scope });
      const result = {
        ownBefore,
        own: own.sort(),
        templateChildIsConcrete,
        longExpiresAt: Number(longExpiresAt),
        parentExpiresAt: Number(parentExpiresAt),
        scopeExpiresAt: Number(scope.expiresAt()?.seconds ?? -1n),
        calleeWithoutScope: calleeWithoutScope.kinds,
        calleeWithScope: calleeWithScope.kinds,
        calleeScopeExpiresAt: calleeWithScope.scopeExpiresAt,
      };
      scope.release();
      walletLong.release();
      walletShort.release();
      walletTemplate.release();
      return result;
    },

    keepScopeCard() {
      const template = templateCard();
      this.keptScope = permissions.deriveScope([template], { lowerPositive: [envGrant] }).unwrap();
      template.release();
    },

    async useKeptScopeCard({ calleeName }) {
      const scope = this.keptScope;
      if (scope === undefined) throw new Error('no kept scope card');
      try {
        return await PermissionCallee.client.get({ name: calleeName }).scopeEnv({ scopeCard: scope });
      } catch (error) {
        return isRemoteCallError(error) ? error.cause.tag : `unexpected: ${errorMessage(error)}`;
      }
    },

    async cardRoundTrip({ calleeName }) {
      const card = permissions.deriveFromWallet({ lowerPositive: [inspectGrant] }).unwrap();
      const id = card.id();
      const returned = await PermissionCallee.client.get({ name: calleeName }).echoCard({ card });
      const template = templateCard();
      const templateId = template.id();
      const returnedTemplate = await PermissionCallee.client
        .get({ name: calleeName })
        .echoTemplate({ card: template });
      const result = {
        movedAfterSend: card.isMoved(),
        sameCard: returned.id() === id,
        returnedInspectable: returned.inspect().isOk(),
        templateMovedAfterSend: template.isMoved(),
        sameTemplate:
          returnedTemplate instanceof permissions.PolymorphicPermissionCard &&
          returnedTemplate.id() === templateId,
        returnedTemplateInspectable: returnedTemplate.inspect().isOk(),
      };
      returned.release();
      returnedTemplate.release();
      return result;
    },

    recipientMismatch({ calleeName }) {
      const card = permissions
        .deriveFromWallet({ lowerPositive: [mismatchedRecipientGrant] })
        .unwrap();
      const result = card.install(calleeHolder(calleeName));
      return {
        error: result.isErr() ? result.unwrapErr().kind : 'installed',
        moved: card.isMoved(),
      };
    },
  },
});

export const PermissionNoAuthority = defineAgent({
  name: 'PermissionNoAuthority',
  id: { name: z.string() },
  methods: {
    deriveFromWallet: method({ input: {}, returns: z.string() }),
  },
});

export const PermissionNoAuthorityImpl = PermissionNoAuthority.implement({
  init: () => ({}),
  methods: {
    deriveFromWallet() {
      const result = permissions.deriveFromWallet({ lowerPositive: [inspectGrant] });
      if (result.isOk()) {
        result.unwrap().release();
        return 'derived';
      }
      return result.unwrapErr().kind;
    },
  },
});
