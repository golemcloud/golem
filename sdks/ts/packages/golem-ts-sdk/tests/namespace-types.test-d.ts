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

// Type-only coverage for the WIT leaf types exposed through the bundled
// `bridge`, `oplog` and `http` namespaces: each must be usable as a type.
// Checked by the package typecheck script; NOT executed by vitest.

import { bridge, http, oplog } from '../dist/index.mjs';

export const quantity: bridge.QuantityValue = { mantissa: 15n, scale: 1, unit: 'g' };
export const quantitySpec: bridge.QuantitySpec = { baseUnit: 'kg', allowedSuffixes: [] };
export const text: bridge.TextRestrictions = { languages: ['en'] };
export const binary: bridge.BinaryRestrictions = { mimeTypes: ['image/png'] };
export const metadata: bridge.MetadataEnvelope | undefined = undefined;
export const tree: bridge.SchemaValueTree | undefined = undefined;
export const segment: http.PathSegment = { tag: 'literal', val: 'counters' };
export const entry: oplog.OplogEntry | undefined = undefined;
export const level: oplog.LogLevel = 'info';
