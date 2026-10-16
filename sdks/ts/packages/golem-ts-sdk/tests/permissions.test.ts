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

import { afterEach, describe, expect, it, vi } from 'vitest';

type PermissionsModule = typeof import('../src/host/permissions');
type PublicPermissionsModule = typeof import('../src/permissions');

interface FakeCard {
  readonly name: string;
  readonly id: bigint;
  readonly parents: bigint[];
  disposed: boolean;
  [Symbol.dispose](): void;
}

function fakeCard(name: string, id: bigint, parents: bigint[] = []): FakeCard {
  return {
    name,
    id,
    parents,
    disposed: false,
    [Symbol.dispose]() {
      this.disposed = true;
    },
  };
}

const cardId = (id: bigint) => ({ uuid: { highBits: 7n, lowBits: id } });

type WalletTag = 'concrete' | 'template' | 'scope';

interface Host {
  wallet: { tag: WalletTag; val: FakeCard }[];
  derive: ReturnType<typeof vi.fn>;
  deriveFromWallet: ReturnType<typeof vi.fn>;
  deriveScope: ReturnType<typeof vi.fn>;
  inspectCard: ReturnType<typeof vi.fn>;
  revokeCard: ReturnType<typeof vi.fn>;
  installCard: ReturnType<typeof vi.fn>;
  validateGrant: ReturnType<typeof vi.fn>;
}

const MODULES = [
  'golem:permissions/types@0.1.0',
  'golem:permissions/inspect@0.1.0',
  'golem:permissions/derive@0.1.0',
  'golem:permissions/revoke@0.1.0',
  'golem:permissions/wallet@0.1.0',
  'golem:permissions/kernel-introspection@0.1.0',
];

async function loadPermissions(): Promise<{
  host: Host;
  permissions: PermissionsModule;
  publicPermissions: PublicPermissionsModule;
}> {
  vi.resetModules();
  const host: Host = {
    wallet: [{ tag: 'template', val: fakeCard('template', 1n) }],
    derive: vi.fn((parent: FakeCard) => fakeCard('child', 3n, [parent.id])),
    deriveFromWallet: vi.fn(() => fakeCard('wallet-derived', 2n, [1n])),
    deriveScope: vi.fn((parents: FakeCard[]) =>
      fakeCard(
        'scope',
        4n,
        parents.map((parent) => parent.id),
      ),
    ),
    inspectCard: vi.fn(() => ({
      lowerPositive: [
        { class_: 'env', owner: '*/*/*/*/*', recipient: '*', verb: 'read', resourceId: 'A' },
      ],
      lowerNegative: [],
      upperPositive: [],
      upperNegative: [],
    })),
    revokeCard: vi.fn(() => 2),
    installCard: vi.fn(),
    validateGrant: vi.fn(),
  };
  vi.doMock('golem:permissions/types@0.1.0', () => ({
    id: (card: FakeCard) => cardId(card.id),
    parents: (card: FakeCard) => card.parents.map(cardId),
    expiresAt: (card: FakeCard) =>
      card.name === 'child'
        ? { seconds: 1_700_000_000n, nanoseconds: 5 }
        : card.name === 'scope'
          ? { seconds: 1_600_000_000n, nanoseconds: 7 }
          : undefined,
  }));
  vi.doMock('golem:permissions/inspect@0.1.0', () => ({ inspectCard: host.inspectCard }));
  vi.doMock('golem:permissions/derive@0.1.0', () => ({
    derive: host.derive,
    deriveFromWallet: host.deriveFromWallet,
    deriveScope: host.deriveScope,
  }));
  vi.doMock('golem:permissions/revoke@0.1.0', () => ({ revokeCard: host.revokeCard }));
  vi.doMock('golem:permissions/wallet@0.1.0', () => ({
    selfWallet: () => host.wallet,
    selfVersion: () => ({ walletIdHash: new Uint8Array([1, 2]), generation: 9n }),
    installCard: host.installCard,
  }));
  vi.doMock('golem:permissions/kernel-introspection@0.1.0', () => ({
    listModules: () => [{ className: 'env', verbs: ['read'] }],
    validateGrant: host.validateGrant,
  }));
  return {
    host,
    permissions: await import('../src/host/permissions'),
    publicPermissions: await import('../src/permissions'),
  };
}

const envGrant = {
  class: 'env',
  owner: '*/*/*/*/*',
  recipient: '*',
  verb: 'read',
  resourceId: 'SCOPE_ENV',
} as const;
const grant = (resourceId: string) => ({ ...envGrant, resourceId });
const rawGrant = (resourceId: string) => ({
  class_: 'env',
  owner: '*/*/*/*/*',
  recipient: '*',
  verb: 'read',
  resourceId,
});

function walletCard<Tag extends WalletTag>(
  permissions: PermissionsModule,
  tag: Tag,
): Extract<ReturnType<PermissionsModule['wallet']>[number], { tag: Tag }>['card'] {
  const entry = permissions.wallet().find((card) => card.tag === tag);
  if (entry === undefined) throw new Error(`no ${tag} wallet card`);
  return entry.card as never;
}

afterEach(() => {
  for (const module of MODULES) vi.doUnmock(module);
  vi.resetModules();
});

describe('permission cards', () => {
  it('borrows cards for inspection and interns card IDs for equality', async () => {
    const { permissions } = await loadPermissions();
    const template = walletCard(permissions, 'template');
    const parent = permissions.deriveFromWallet({ upperPositive: [envGrant] }).unwrap();
    const child = parent.derive({}).unwrap();

    expect(parent.parents()).toEqual([template.id()]);
    expect(child.parents()[0]).toBe(parent.id());
    expect(child.parents()[0].equals(parent.id())).toBe(true);
    expect(child.id().equals(parent.id())).toBe(false);
    expect(new Set([parent.id(), parent.id(), child.id()]).size).toBe(2);
    expect(String(parent.id())).toBe('CardId(<redacted>)');
    expect(`${parent.id()}`).toBe('CardId(<redacted>)');
    expect(JSON.stringify({ id: parent.id() })).toBe('{"id":"CardId(<redacted>)"}');
    expect(child.expiresAt()).toEqual({ seconds: 1_700_000_000n, nanoseconds: 5 });
    expect(parent.expiresAt()).toBeUndefined();
    expect(parent.inspect().unwrap()).toEqual({
      lowerPositive: [grant('A')],
      lowerNegative: [],
      upperPositive: [],
      upperNegative: [],
    });
    expect(parent.isMoved()).toBe(false);
  });

  it('sends each bounds list to its own WIT parameter', async () => {
    const { host, permissions } = await loadPermissions();
    const parent = permissions
      .deriveFromWallet(
        {
          lowerPositive: [grant('lp')],
          lowerNegative: [grant('ln')],
          upperPositive: [grant('up')],
          upperNegative: [grant('un')],
        },
        permissions.Lifespan.iso8601('P1D'),
      )
      .unwrap();
    expect(host.deriveFromWallet).toHaveBeenCalledWith(
      [rawGrant('lp')],
      [rawGrant('ln')],
      [rawGrant('up')],
      [rawGrant('un')],
      { iso8601: 'P1D' },
    );

    parent.derive({ upperPositive: [grant('keep')], lowerNegative: [grant('deny')] });
    expect(host.derive).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'wallet-derived' }),
      [],
      [rawGrant('deny')],
      [rawGrant('keep')],
      [],
      undefined,
    );

    permissions.deriveScope([parent], { upperNegative: [grant('scope-deny')] });
    expect(host.deriveScope).toHaveBeenCalledWith(
      [expect.objectContaining({ name: 'wallet-derived' })],
      [],
      [],
      [],
      [rawGrant('scope-deny')],
    );
    expect(parent.isMoved()).toBe(false);
  });

  it('builds ISO-8601 lifespans from SDK durations', async () => {
    const { permissions } = await loadPermissions();
    const { Duration } = await import('../src/host/retryBuilder');
    expect(permissions.Lifespan.of(Duration.milliseconds(1500)).iso8601).toBe('PT1.5S');
    expect(permissions.Lifespan.of(Duration.hours(2)).iso8601).toBe('PT7200S');
    expect(permissions.Lifespan.of(Duration.nanoseconds(1)).iso8601).toBe('PT0.000000001S');
    expect(() => permissions.Lifespan.of(Duration.nanoseconds(0))).toThrow(RangeError);
    expect(permissions.Lifespan.iso8601('P1DT2H').iso8601).toBe('P1DT2H');
    const parent = permissions.deriveFromWallet({}).unwrap();
    expect(() =>
      parent.derive({}, { iso8601: 'PT1S' } as unknown as InstanceType<
        PermissionsModule['Lifespan']
      >),
    ).toThrow(/expected a Lifespan/);
  });

  it('maps host permission errors into Result.err and rethrows traps', async () => {
    const { host, permissions } = await loadPermissions();
    host.deriveFromWallet.mockImplementationOnce(() => {
      throw { tag: 'upper-bound-too-broad', val: 'grant exceeds the wallet' };
    });
    const failed = permissions.deriveFromWallet({ upperPositive: [envGrant] });
    expect(failed.isErr()).toBe(true);
    const error = failed.unwrapErr();
    expect(error).toBeInstanceOf(permissions.PermissionError);
    expect(error).toMatchObject({
      kind: 'upper-bound-too-broad',
      details: 'grant exceeds the wallet',
    });

    const trap = new Error('host trap');
    host.deriveFromWallet.mockImplementationOnce(() => {
      throw trap;
    });
    expect(() => permissions.deriveFromWallet({})).toThrow(trap);

    host.deriveFromWallet.mockImplementationOnce(() => {
      throw { tag: 'card-polymorphic-not-usable', val: 'removed from the WIT' };
    });
    expect(() => permissions.deriveFromWallet({})).toThrow();

    host.validateGrant.mockImplementationOnce(() => {
      throw { tag: 'invalid-verb', val: 'fly' };
    });
    expect(permissions.validateGrant({ ...envGrant, verb: 'fly' }).unwrapErr().kind).toBe(
      'invalid-verb',
    );
    expect(host.validateGrant).toHaveBeenCalledWith({ ...rawGrant('SCOPE_ENV'), verb: 'fly' });
  });

  it('pins the permission-error cases to the WIT variant', async () => {
    const { permissions } = await loadPermissions();
    expect(permissions.PERMISSION_ERROR_KINDS).toHaveLength(12);
    expect(new Set(permissions.PERMISSION_ERROR_KINDS).size).toBe(12);
    expect(permissions.PERMISSION_ERROR_KINDS).toContain('install-recipient-mismatch');
  });

  it('moves a card before revoking it, even when the host reports an error', async () => {
    const { host, permissions } = await loadPermissions();
    const revoked = permissions.deriveFromWallet({}).unwrap();
    expect(revoked.revoke().unwrap()).toBe(2);
    expect(revoked.isMoved()).toBe(true);
    expect(() => revoked.id()).toThrow(/already been moved/);
    expect(() => revoked.revoke()).toThrow(/already been moved/);
    expect(host.revokeCard).toHaveBeenCalledOnce();

    const denied = permissions.deriveFromWallet({}).unwrap();
    host.revokeCard.mockImplementationOnce(() => {
      throw { tag: 'not-permitted', val: 'no revoke authority' };
    });
    expect(denied.revoke().unwrapErr().kind).toBe('not-permitted');
    expect(denied.isMoved()).toBe(true);
    expect(() => denied.inspect()).toThrow(/already been moved/);
  });

  it('installs a card on an agent holder and moves it on failure', async () => {
    const { host, permissions } = await loadPermissions();
    const { ComponentId } = await import('../src/ids');
    const { Uuid } = await import('../src/uuid');
    const target = permissions.Holder.agent({
      componentId: new ComponentId(new Uuid(11n, 12n)),
      agentId: 'Worker("a")',
    });
    const installed = permissions.deriveFromWallet({}).unwrap();
    expect(installed.install(target).isOk()).toBe(true);
    expect(host.installCard).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'wallet-derived' }),
      {
        tag: 'agent',
        val: { componentId: { uuid: { highBits: 11n, lowBits: 12n } }, agentId: 'Worker("a")' },
      },
    );
    expect(installed.isMoved()).toBe(true);

    const mismatched = permissions.deriveFromWallet({}).unwrap();
    host.installCard.mockImplementationOnce(() => {
      throw { tag: 'install-recipient-mismatch', val: 'no matching recipient' };
    });
    expect(mismatched.install(target).unwrapErr().kind).toBe('install-recipient-mismatch');
    expect(mismatched.isMoved()).toBe(true);

    const kept = permissions.deriveFromWallet({}).unwrap();
    expect(() => kept.install({ tag: 'agent', val: target.val } as never)).toThrow(TypeError);
    expect(kept.isMoved()).toBe(false);
  });

  it('labels each wallet card with its kind and its own card type', async () => {
    const { host, permissions } = await loadPermissions();
    host.wallet = [
      { tag: 'concrete', val: fakeCard('concrete', 11n) },
      { tag: 'template', val: fakeCard('template', 12n) },
      { tag: 'scope', val: fakeCard('scope', 13n, [12n]) },
    ];
    const [concrete, template, scope] = permissions.wallet();
    expect(Object.isFrozen(concrete)).toBe(true);
    if (concrete.tag !== 'concrete' || template.tag !== 'template' || scope.tag !== 'scope') {
      throw new Error('the wallet lost the host order or labels');
    }
    expect(concrete.card).toBeInstanceOf(permissions.PermissionCard);
    expect(concrete.card).not.toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(template.card).toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(template.card).not.toBeInstanceOf(permissions.PermissionCard);
    expect(scope.card).toBeInstanceOf(permissions.ScopeCard);
    expect(scope.card).not.toBeInstanceOf(permissions.PermissionCard);
    expect(scope.card).not.toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(scope.card.parents()).toEqual([template.card.id()]);
    expect(scope.card.expiresAt()).toEqual({ seconds: 1_600_000_000n, nanoseconds: 7 });

    concrete.card.release();
    template.card.release();
    scope.card.release();
    expect(host.wallet.map((entry) => entry.val.disposed)).toEqual([true, true, true]);
    expect(scope.card.isMoved()).toBe(true);
    expect(() => scope.card.expiresAt()).toThrow(/already been released/);
  });

  it('releases the adopted wallet cards when a later entry fails', async () => {
    const { host, permissions } = await loadPermissions();
    const shared = fakeCard('shared', 21n);
    const held = permissions.deriveFromWallet({}).unwrap();
    host.deriveFromWallet.mockImplementationOnce(() => shared);
    const owner = permissions.deriveFromWallet({}).unwrap();
    const first = fakeCard('first', 22n);
    const last = fakeCard('last', 23n);
    host.wallet = [
      { tag: 'concrete', val: first },
      { tag: 'template', val: shared },
      { tag: 'concrete', val: last },
    ];
    expect(() => permissions.wallet()).toThrow(/already owned/);
    expect(first.disposed).toBe(true);
    expect(last.disposed).toBe(true);
    // The failing entry belongs to another card, which keeps it.
    expect(shared.disposed).toBe(false);
    expect(owner.isMoved()).toBe(false);
    expect(held.isMoved()).toBe(false);

    host.wallet = [{ tag: 'other' as never, val: fakeCard('unknown', 24n) }];
    expect(() => permissions.wallet()).toThrow(/unknown wallet card kind 'other'/);
  });

  it('returns owned wallet cards that the caller can release', async () => {
    const { host, permissions } = await loadPermissions();
    const card = walletCard(permissions, 'template');
    card.release();
    expect(host.wallet[0].val.disposed).toBe(true);
    expect(card.isMoved()).toBe(true);
    expect(() => card.release()).toThrow(/already been moved/);
    expect(permissions.walletVersion()).toEqual({
      walletIdHash: new Uint8Array([1, 2]),
      generation: 9n,
    });
    expect(permissions.listModules()).toEqual([{ className: 'env', verbs: ['read'] }]);
  });

  it('borrows scope parents and rejects a moved parent before the host call', async () => {
    const { host, permissions } = await loadPermissions();
    const parent = permissions.deriveFromWallet({}).unwrap();
    const scope = permissions.deriveScope([parent], { upperPositive: [envGrant] }).unwrap();
    expect(scope.parents()).toEqual([parent.id()]);
    expect(parent.isMoved()).toBe(false);
    parent.release();
    expect(() => permissions.deriveScope([parent], {})).toThrow(/already been moved/);
    expect(host.deriveScope).toHaveBeenCalledOnce();
  });

  it('derives a scope card that has no persistent-card operations', async () => {
    const { host, permissions } = await loadPermissions();
    const parent = permissions.deriveFromWallet({}).unwrap();
    const scope = permissions.deriveScope([parent], { lowerPositive: [envGrant] }).unwrap();
    expect(scope).toBeInstanceOf(permissions.ScopeCard);
    expect(scope).not.toBeInstanceOf(permissions.PermissionCard);
    for (const operation of ['revoke', 'install', 'derive']) {
      expect(operation in scope).toBe(false);
    }
    // The host reports the earliest expiry of the scope card's root cards.
    expect(scope.expiresAt()).toEqual({ seconds: 1_600_000_000n, nanoseconds: 7 });
    expect(Object.isFrozen(scope.expiresAt())).toBe(true);
    expect(scope.inspect().unwrap().lowerPositive).toEqual([grant('A')]);
    expect(() => JSON.stringify(scope)).toThrow(/cannot be serialized/);

    const nested = permissions.deriveScope([scope, parent], {}).unwrap();
    expect(host.deriveScope).toHaveBeenLastCalledWith(
      [
        expect.objectContaining({ name: 'scope' }),
        expect.objectContaining({ name: 'wallet-derived' }),
      ],
      [],
      [],
      [],
      [],
    );
    nested.release();
    expect(nested.isMoved()).toBe(true);
    expect(() => nested.id()).toThrow(/already been released/);
    expect(() => permissions.deriveScope([nested], {})).toThrow(/already been released/);
    expect(scope.isMoved()).toBe(false);
  });

  it('gives template cards the persistent operations, and derives concrete children', async () => {
    const { host, permissions } = await loadPermissions();
    const template = walletCard(permissions, 'template');
    expect(template.inspect().unwrap().lowerPositive).toEqual([grant('A')]);
    expect(template.expiresAt()).toBeUndefined();

    const child = template.derive({ lowerPositive: [envGrant] }).unwrap();
    expect(child).toBeInstanceOf(permissions.PermissionCard);
    expect(child).not.toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(child.parents()).toEqual([template.id()]);

    const scope = permissions.deriveScope([template], { lowerPositive: [envGrant] }).unwrap();
    expect(host.deriveScope).toHaveBeenLastCalledWith(
      [expect.objectContaining({ name: 'template' })],
      [rawGrant('SCOPE_ENV')],
      [],
      [],
      [],
    );
    expect(scope.parents()).toEqual([template.id()]);
    expect(template.isMoved()).toBe(false);

    host.revokeCard.mockImplementationOnce(() => {
      throw { tag: 'not-permitted', val: 'no revoke authority' };
    });
    expect(template.revoke().unwrapErr().kind).toBe('not-permitted');
    expect(template.isMoved()).toBe(true);
    expect(() => template.derive({})).toThrow(/already been moved/);

    const { ComponentId } = await import('../src/ids');
    const { Uuid } = await import('../src/uuid');
    host.wallet = [{ tag: 'template', val: fakeCard('template', 1n) }];
    const installed = walletCard(permissions, 'template');
    host.installCard.mockImplementationOnce(() => {
      throw { tag: 'install-recipient-mismatch', val: 'no matching recipient' };
    });
    const target = permissions.Holder.agent({
      componentId: new ComponentId(new Uuid(1n, 2n)),
      agentId: 'Worker("a")',
    });
    expect(installed.install(target).unwrapErr().kind).toBe('install-recipient-mismatch');
    expect(installed.isMoved()).toBe(true);
    expect(() => installed.install(target)).toThrow(/already been moved/);
  });

  it('compares cards by identity in structural equality', async () => {
    const { permissions } = await loadPermissions();
    await import('../src/index');
    const { deepEqual } = await import('../src/internal/schema-model/model');
    const { compileSchema } = await import('../src/schema/adapter');
    const { sourceValueIsCanonical } = await import('../src/schema/codec');
    const { s } = await import('../src/schema/markers');
    const { z } = await import('zod');
    const first = permissions.deriveFromWallet({}).unwrap();
    const second = permissions.deriveFromWallet({}).unwrap();
    expect(deepEqual(first, first)).toBe(true);
    expect(deepEqual(first, second)).toBe(false);
    expect(deepEqual({ card: first }, { card: second })).toBe(false);

    const codec = compileSchema(
      z.object({
        a: s.permissionCard({ polymorphic: false }),
        b: s.permissionCard({ polymorphic: false }),
      }),
    );
    const source = { a: first, b: second };
    expect(sourceValueIsCanonical(codec, source, codec.toValue(source))).toBe(true);
    expect(sourceValueIsCanonical(codec, source, codec.toValue({ a: second, b: first }))).toBe(
      false,
    );
  });
});

describe('permission card non-forgeability', () => {
  it('has no public constructor and no forged instances', async () => {
    const { permissions, publicPermissions } = await loadPermissions();
    const { GuestPermissionCardHandle } =
      await import('../src/internal/schema-model/permissionCardHandle');
    const PermissionCard = publicPermissions.PermissionCard as unknown as new (
      ...args: unknown[]
    ) => unknown;
    expect(
      () => new PermissionCard(Symbol('golem:permission-card internal capability'), {}),
    ).toThrow(/internal SDK operation/);
    expect(() => new PermissionCard(undefined, fakeCard('forged', 99n))).toThrow(
      /internal SDK operation/,
    );
    expect(
      () =>
        new (GuestPermissionCardHandle as unknown as new (...args: unknown[]) => unknown)(
          Symbol('golem:permission-card internal capability'),
          fakeCard('forged', 99n),
        ),
    ).toThrow(/internal SDK operation/);

    const forged = Object.create(permissions.PermissionCard.prototype) as InstanceType<
      PermissionsModule['PermissionCard']
    >;
    expect(() => forged.id()).toThrow('invalid permission card');
    expect(() => forged.revoke()).toThrow('invalid permission card');
    expect(() => permissions.deriveScope([forged], {})).toThrow('invalid permission card');

    const PolymorphicPermissionCard =
      publicPermissions.PolymorphicPermissionCard as unknown as new (...args: unknown[]) => unknown;
    expect(
      () => new PolymorphicPermissionCard(Symbol('golem:permission-card internal capability'), {}),
    ).toThrow(/internal SDK operation/);
    expect(() => new PolymorphicPermissionCard(undefined, fakeCard('forged', 99n))).toThrow(
      /internal SDK operation/,
    );
    // A card's kind cannot change after construction.
    const concrete = permissions.deriveFromWallet({}).unwrap();
    expect(() =>
      Object.setPrototypeOf(concrete, permissions.PolymorphicPermissionCard.prototype),
    ).toThrow(TypeError);
    expect(concrete).toBeInstanceOf(permissions.PermissionCard);
    expect(concrete).not.toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(Object.isFrozen(concrete)).toBe(true);

    const forgedTemplate = Object.create(
      permissions.PolymorphicPermissionCard.prototype,
    ) as InstanceType<PermissionsModule['PolymorphicPermissionCard']>;
    expect(() => forgedTemplate.id()).toThrow('invalid permission card');
    expect(() => forgedTemplate.install(undefined as never)).toThrow(TypeError);
    expect(() => forgedTemplate.release()).toThrow('invalid permission card');
    expect(() => permissions.deriveScope([forgedTemplate], {})).toThrow('invalid permission card');
    expect(() => permissions.deriveScope([{} as never], {})).toThrow(TypeError);

    const CardId = publicPermissions.CardId as unknown as new (...args: unknown[]) => unknown;
    expect(() => new CardId(Symbol('golem:card-id internal capability'), '7:1')).toThrow(
      /internal SDK operation/,
    );
  });

  it('cannot serialize cards or card IDs', async () => {
    const { host, permissions } = await loadPermissions();
    const card = permissions.deriveFromWallet({}).unwrap();
    expect(() => JSON.stringify(card)).toThrow(/cannot be serialized/);
    expect(Object.keys(card)).toEqual([]);
    const template = walletCard(permissions, 'template');
    expect(() => JSON.stringify({ template })).toThrow(/cannot be serialized/);
    expect(Object.keys(template)).toEqual([]);
    host.wallet = [{ tag: 'concrete', val: fakeCard('concrete', 5n) }];
    expect(() => JSON.stringify(permissions.wallet())).toThrow(/cannot be serialized/);
  });

  it('keeps SDK-internal card conversions out of the public namespace', async () => {
    const { publicPermissions } = await loadPermissions();
    expect(Object.keys(publicPermissions).sort()).toEqual([
      'CardId',
      'Holder',
      'Lifespan',
      'PERMISSION_ERROR_KINDS',
      'PermissionCard',
      'PermissionError',
      'PolymorphicPermissionCard',
      'ScopeCard',
      'deriveFromWallet',
      'deriveScope',
      'listModules',
      'validateGrant',
      'wallet',
      'walletVersion',
    ]);
  });

  it('fails loudly after a card moves through a schema value', async () => {
    const { permissions } = await loadPermissions();
    const { compileSchema } = await import('../src/schema/adapter');
    const { s } = await import('../src/schema/markers');
    const { schemaValueToWit } = await import('../src/internal/schema-model');
    const codec = compileSchema(s.permissionCard({ polymorphic: false }));
    const card = permissions.deriveFromWallet({}).unwrap();

    expect(() => codec.toValue({ id: 'not-a-card' })).toThrow(/Expected a PermissionCard/);
    const tree = schemaValueToWit(codec.toValue(card));
    expect(tree.valueNodes[tree.root]).toMatchObject({ tag: 'permission-card-handle' });
    expect(card.isMoved()).toBe(true);
    expect(() => card.derive({})).toThrow(/already been moved/);
    expect(() => schemaValueToWit(codec.toValue(card))).toThrow(/already transferred/);

    const templateCodec = compileSchema(s.permissionCard({ polymorphic: true }));
    const template = walletCard(permissions, 'template');
    const templateTree = schemaValueToWit(templateCodec.toValue(template));
    expect(templateTree.valueNodes[templateTree.root]).toMatchObject({
      tag: 'permission-card-handle',
    });
    expect(template.isMoved()).toBe(true);
    expect(() => template.inspect()).toThrow(/already been moved/);
    expect(() => schemaValueToWit(templateCodec.toValue(template))).toThrow(/already transferred/);
  });

  it('keeps each schema flag to its own card type', async () => {
    const { host, permissions } = await loadPermissions();
    const { compileSchema } = await import('../src/schema/adapter');
    const { s } = await import('../src/schema/markers');
    const { schemaValueFromWit, schemaValueToWit } = await import('../src/internal/schema-model');
    const { PermissionCard: RawPermissionCard } = await import('golem:core/types@2.0.0');
    const concreteCodec = compileSchema(s.permissionCard({ polymorphic: false }));
    const templateCodec = compileSchema(s.permissionCard({ polymorphic: true }));
    // Only instances of the host resource class lift from a wire value.
    host.deriveFromWallet.mockImplementationOnce(() =>
      Object.assign(Object.create(RawPermissionCard.prototype), fakeCard('wire', 5n, [1n])),
    );
    const concrete = permissions.deriveFromWallet({}).unwrap();
    const template = walletCard(permissions, 'template');

    expect(() => templateCodec.toValue(concrete)).toThrow(/Expected a PolymorphicPermissionCard/);
    expect(() => concreteCodec.toValue(template)).toThrow(/Expected a PermissionCard/);
    expect(concrete.isMoved()).toBe(false);
    expect(template.isMoved()).toBe(false);
    expect(s.permissionCard({ polymorphic: true })['~standard'].validate(concrete)).toHaveProperty(
      'issues',
    );
    expect(s.permissionCard({ polymorphic: false })['~standard'].validate(template)).toHaveProperty(
      'issues',
    );
    expect(() => s.permissionCard({ polymorphic: 'yes' } as never)).toThrow(TypeError);

    // The host does not check the kind: a received card becomes the declared type.
    const decoded = templateCodec.fromValue(
      schemaValueFromWit(schemaValueToWit(concreteCodec.toValue(concrete))),
    );
    expect(concrete.isMoved()).toBe(true);
    expect(decoded).toBeInstanceOf(permissions.PolymorphicPermissionCard);
    expect(decoded.parents()).toEqual([template.id()]);

    // A local decode would put the card's handle into a second card, also of the other kind.
    expect(() => templateCodec.fromValue(templateCodec.toValue(template))).toThrow(
      /already held by another card/,
    );
    expect(() => concreteCodec.fromValue(templateCodec.toValue(template))).toThrow(
      /already held by another card/,
    );
    expect(template.isMoved()).toBe(false);
    const other = permissions.deriveFromWallet({}).unwrap();
    expect(() => templateCodec.fromValue(concreteCodec.toValue(other))).toThrow(
      /already held by another card/,
    );
    expect(other.isMoved()).toBe(false);
  });
});
