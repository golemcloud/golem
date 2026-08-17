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

// Capability key gating the privileged operations that move or re-wrap an
// owned `permission-card` resource. This module is intentionally not exported
// from the package entry point or the schema-model barrel.
export const PERMISSION_CARD_INTERNAL: unique symbol = Symbol(
  'golem:permission-card internal capability',
);

/** The type of the {@link PERMISSION_CARD_INTERNAL} capability key. */
export type PermissionCardInternal = typeof PERMISSION_CARD_INTERNAL;
