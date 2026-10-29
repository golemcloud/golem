// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { Secret as RawSecret } from 'golem:core/types@2.0.0';
import {
  schemaGraphsEquivalent,
  type SchemaGraph,
  type SchemaValue,
  type TypedSchemaValue,
  v,
} from '../internal/schema-model';
import { PERMISSION_CARD_INTERNAL } from '../internal/schema-model/permissionCardInternal';
import { QUOTA_INTERNAL } from '../internal/schema-model/quotaInternal';
import { SECRET_INTERNAL } from '../internal/schema-model/secretInternal';
import {
  adoptGuestSecretHandle,
  releaseGuestSecretHandle,
} from '../internal/schema-model/secretHandle';
import { schemaValueConforms } from '../internal/tool/validation';
import {
  quotaTokenFromSchemaValueInternal,
  quotaTokenToSchemaValueInternal,
  QuotaToken,
} from '../host/quota';
import {
  PermissionCard,
  permissionCardFromSchemaHandleInternal,
  permissionCardToSchemaValueInternal,
  PolymorphicPermissionCard,
  polymorphicPermissionCardFromSchemaHandleInternal,
} from '../host/permissions';

export type SecretHandle = RawSecret;
export { PermissionCard, PolymorphicPermissionCard, QuotaToken };
export { ScopeCard } from '../host/permissions';

export function secretHandleToSchemaValue(value: SecretHandle): SchemaValue {
  return v.secret(adoptGuestSecretHandle(SECRET_INTERNAL, value));
}

export function secretHandleFromSchemaValue(value: SchemaValue): SecretHandle {
  if (value.tag !== 'secret') {
    throw new Error(`Expected a secret schema value, got '${value.tag}'`);
  }
  const raw = releaseGuestSecretHandle(SECRET_INTERNAL, value.handle);
  if (raw === undefined) {
    throw new Error('secret handle was already consumed; an owned secret can only be decoded once');
  }
  return raw;
}

export function quotaTokenToSchemaValue(value: QuotaToken): SchemaValue {
  return quotaTokenToSchemaValueInternal(QUOTA_INTERNAL, value);
}

export function quotaTokenFromSchemaValue(value: SchemaValue): QuotaToken {
  return quotaTokenFromSchemaValueInternal(QUOTA_INTERNAL, value);
}

export function permissionCardToSchemaValue(value: PermissionCard): SchemaValue {
  if (!(value instanceof PermissionCard)) {
    throw new TypeError('Expected a PermissionCard for WIT permission-card { polymorphic: false }');
  }
  return permissionCardToSchemaValueInternal(PERMISSION_CARD_INTERNAL, value);
}

export function permissionCardFromSchemaValue(value: SchemaValue): PermissionCard {
  if (value.tag !== 'permission-card') {
    throw new Error(`Expected a permission-card schema value, got '${value.tag}'`);
  }
  return permissionCardFromSchemaHandleInternal(PERMISSION_CARD_INTERNAL, value.handle);
}

export function polymorphicPermissionCardToSchemaValue(
  value: PolymorphicPermissionCard,
): SchemaValue {
  if (!(value instanceof PolymorphicPermissionCard)) {
    throw new TypeError(
      'Expected a PolymorphicPermissionCard for WIT permission-card { polymorphic: true }',
    );
  }
  return permissionCardToSchemaValueInternal(PERMISSION_CARD_INTERNAL, value);
}

export function polymorphicPermissionCardFromSchemaValue(
  value: SchemaValue,
): PolymorphicPermissionCard {
  if (value.tag !== 'permission-card') {
    throw new Error(`Expected a permission-card schema value, got '${value.tag}'`);
  }
  return polymorphicPermissionCardFromSchemaHandleInternal(PERMISSION_CARD_INTERNAL, value.handle);
}

export function typedSchemaValueConforms(
  expectedGraph: SchemaGraph,
  typed: TypedSchemaValue,
): boolean {
  return (
    schemaGraphsEquivalent(typed.graph, expectedGraph) &&
    schemaValueConforms(expectedGraph, expectedGraph.root, typed.value)
  );
}
