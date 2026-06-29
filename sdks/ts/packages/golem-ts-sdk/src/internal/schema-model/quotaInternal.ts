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

// Capability key gating the privileged quota-token operations that move or
// re-wrap the underlying owned `own<quota-token>` resource.
//
// A `quota-token` is an unforgeable, affine capability: a guest may hold and
// transfer it but must never extract the raw owned handle, re-wrap it, or
// duplicate it — doing so would let it forge or double-spend the capability.
// The privileged operations (`GuestQuotaTokenHandle.fromRaw`,
// `QuotaToken._toSchemaValue` / `_fromSchemaValue` / `_fromHandle`) all require
// this key as a witness, so only SDK-internal modules that can import it may
// call them.
//
// This module is intentionally NOT re-exported from the package's public entry
// point (`src/index.ts`) nor from the `internal/schema-model` barrel, so guest
// code cannot obtain the symbol. A `Symbol()` is unique and unguessable, so it
// cannot be forged at runtime either.
export const QUOTA_INTERNAL: unique symbol = Symbol('golem:quota internal capability');

/** The type of the {@link QUOTA_INTERNAL} capability key. */
export type QuotaInternal = typeof QUOTA_INTERNAL;
