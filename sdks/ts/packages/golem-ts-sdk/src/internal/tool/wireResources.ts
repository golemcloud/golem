// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import * as secret from '../schema-model/secretHandle';
import * as quota from '../schema-model/quotaTokenHandle';
import * as permission from '../schema-model/permissionCardHandle';
import { SECRET_INTERNAL } from '../schema-model/secretInternal';
import { QUOTA_INTERNAL } from '../schema-model/quotaInternal';
import { PERMISSION_CARD_INTERNAL } from '../schema-model/permissionCardInternal';

interface ResourceOperations {
  check(raw: object, node: object): void;
  lift(raw: object, node: object): void;
  adopt(raw: object): { commit(node: object): void; rollback(): void };
}

function operations<Raw extends object, Handle, Key>(
  key: Key,
  check: (key: Key, raw: Raw, node: object) => void,
  lift: (key: Key, raw: Raw, node: object) => Handle,
  adopt: (key: Key, raw: Raw) => Handle,
  release: (key: Key, handle: Handle) => Raw | undefined,
  transfer: (key: Key, handle: Handle, node: object) => Raw | undefined,
): ResourceOperations {
  return {
    check: (raw, node) => check(key, raw as Raw, node),
    lift: (raw, node) => {
      release(key, lift(key, raw as Raw, node));
    },
    adopt(raw) {
      const handle = adopt(key, raw as Raw);
      return {
        commit: (node) => {
          transfer(key, handle, node);
        },
        rollback: () => {
          release(key, handle);
        },
      };
    },
  };
}

export const wireResources: Partial<Record<string, ResourceOperations>> = {
  'secret-value': operations(
    SECRET_INTERNAL,
    secret.assertGuestSecretHandleCanLiftFromWire,
    secret.liftGuestSecretHandleFromWire,
    secret.createGuestSecretHandle,
    secret.releaseGuestSecretHandle,
    secret.takeGuestSecretHandleToWire,
  ),
  'quota-token-handle': operations(
    QUOTA_INTERNAL,
    quota.assertGuestQuotaTokenHandleCanLiftFromWire,
    quota.liftGuestQuotaTokenHandleFromWire,
    quota.createGuestQuotaTokenHandle,
    quota.releaseGuestQuotaTokenHandle,
    quota.takeGuestQuotaTokenHandleToWire,
  ),
  'permission-card-handle': operations(
    PERMISSION_CARD_INTERNAL,
    permission.assertGuestPermissionCardHandleCanLiftFromWire,
    permission.liftGuestPermissionCardHandleFromWire,
    permission.createGuestPermissionCardHandle,
    permission.releaseGuestPermissionCardHandle,
    permission.takeGuestPermissionCardHandleToWire,
  ),
};
