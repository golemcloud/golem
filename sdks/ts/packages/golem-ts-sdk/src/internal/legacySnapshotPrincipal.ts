// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { parseUuid } from 'golem:core/types@2.0.0';
import { SnapshotError } from '../snapshot';

/**
 * Checks the principal of a 1.6 snapshot envelope, and discards it.
 *
 * Snapshots written by 1.6 SDKs carry the 1.6 principal. A 2.0 agent reads such a snapshot with
 * the same strictness as before (a malformed principal still fails the load) and then drops the
 * principal: the caller of an invocation comes from the host, never from saved state.
 *
 * With `strict`, the shape is checked field by field (the multipart form); without it, only
 * what is needed to read the value is checked.
 */
export function validateLegacySnapshotPrincipal(obj: any, strict = false): void {
  if (strict) {
    if (obj === null || typeof obj !== 'object' || Array.isArray(obj))
      throw new SnapshotError('invalid snapshot principal');
    if (obj.tag !== 'anonymous') {
      if (obj.val === null || typeof obj.val !== 'object' || Array.isArray(obj.val))
        throw new SnapshotError('invalid snapshot principal value');
      const val = obj.val;
      if (
        obj.tag === 'agent' &&
        (typeof val.componentId !== 'string' || typeof val.agentId !== 'string')
      )
        throw new SnapshotError('invalid agent principal');
      if (obj.tag === 'golem-user' && typeof val.accountId !== 'string')
        throw new SnapshotError('invalid golem-user principal');
      if (obj.tag === 'oidc') {
        for (const key of ['sub', 'issuer', 'claims']) {
          if (typeof val[key] !== 'string')
            throw new SnapshotError(`invalid oidc principal ${key}`);
        }
        for (const key of [
          'email',
          'name',
          'givenName',
          'familyName',
          'picture',
          'preferredUsername',
        ]) {
          if (val[key] != null && typeof val[key] !== 'string')
            throw new SnapshotError(`invalid oidc principal ${key}`);
        }
        if (val.emailVerified != null && typeof val.emailVerified !== 'boolean')
          throw new SnapshotError('invalid oidc principal emailVerified');
      }
    }
  }
  switch (obj.tag) {
    case 'anonymous':
      return;
    case 'agent':
      parseUuid(obj.val.componentId);
      return;
    case 'golem-user':
      parseUuid(obj.val.accountId);
      return;
    case 'oidc':
      if (
        !obj.val ||
        typeof obj.val.sub !== 'string' ||
        typeof obj.val.issuer !== 'string' ||
        typeof obj.val.claims !== 'string'
      ) {
        throw new Error('Missing required fields (sub, issuer, claims) in oidc principal');
      }
      return;
    default:
      throw new Error(`Unknown principal tag: ${obj.tag}`);
  }
}
