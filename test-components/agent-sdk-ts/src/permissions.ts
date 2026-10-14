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
  type PermissionCard,
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

/** The wallet card that carries the caller's concrete env grant. Releases the other wallet cards. */
function templateCard(): PermissionCard {
  let template: PermissionCard | undefined;
  for (const card of permissions.wallet()) {
    const view = template === undefined ? card.inspect() : undefined;
    if (
      view !== undefined &&
      view.isOk() &&
      view.unwrap().lowerPositive.some((grant) => sameGrant(grant, envGrant))
    ) {
      template = card;
    } else {
      card.release();
    }
  }
  if (template === undefined) throw new Error('the caller wallet has no card with the env grant');
  return template;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const PermissionCallee = defineAgent({
  name: 'PermissionCallee',
  id: { name: z.string() },
  methods: {
    scopeEnv: method({ input: {}, returns: z.string() }),
    walletSize: method({ input: {}, returns: z.number() }),
    echoCard: method({
      input: { card: s.permissionCard({ polymorphic: false }) },
      returns: s.permissionCard({ polymorphic: false }),
    }),
  },
});

export const PermissionCalleeImpl = PermissionCallee.implement({
  init: () => ({}),
  methods: {
    scopeEnv() {
      return getEnvironment().find(([key]) => key === SCOPE_ENV)?.[1] ?? ABSENT;
    },
    walletSize() {
      const cards = permissions.wallet();
      for (const card of cards) card.release();
      return cards.length;
    },
    echoCard({ card }) {
      if (card.isMoved()) throw new Error('the received card is not usable');
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
        walletBefore: z.number(),
        installed: z.boolean(),
        movedError: z.string(),
        walletAfterInstall: z.number(),
        envAfterInstall: z.string(),
        revokedCount: z.number(),
        walletAfterRevoke: z.number(),
        envAfterRevoke: z.string(),
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
      const walletBefore = await callee.walletSize();
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
      const walletAfterInstall = await callee.walletSize();
      const envAfterInstall = await callee.scopeEnv();
      const keptChild = parent.derive({ lowerPositive: [envGrant] }).unwrap();
      const revokedCount = parent.revoke().unwrap();
      const walletAfterRevoke = await callee.walletSize();
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
      const result = {
        movedAfterSend: card.isMoved(),
        sameCard: returned.id() === id,
        returnedInspectable: returned.inspect().isOk(),
      };
      returned.release();
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
