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

// Per-vendor schema-walker registry. Standard Schema standardises validation +
// type inference but exposes NO structure, so recovering the WIT type + value
// codec is library-specific: each vendor registers a `SchemaWalker` keyed on
// `~standard.vendor`. `compileSchema` is the recursive entry point the walkers
// use for child schemas.
//
// Walkers MUST introspect the passed schema OBJECT (duck-typing its runtime
// structure) and must NOT statically `import` the schema library, so no schema
// library is baked into the SDK / WASM — it lives only in the component bundle.

import { FluentCodec, SchemaWalker } from './codec';
import { isStandardSchema } from './standardSchema';
import { isMarkerSchema, WIT_MARKER } from './markers';

const walkers = new Map<string, SchemaWalker>();

export function registerSchemaWalker(vendor: string, walker: SchemaWalker): void {
  walkers.set(vendor, walker);
}

/** Registered vendors (for diagnostics). */
export function registeredVendors(): string[] {
  return Array.from(walkers.keys());
}

/**
 * Compile a Standard Schema value into a {@link FluentCodec} via its vendor
 * walker. Recursive entry point: walkers call it for child schemas.
 */
export function compileSchema(schema: unknown): FluentCodec {
  // Markers carry a hidden `WIT_MARKER` brand (a `FluentCodec`-builder) so users
  // can express WIT kinds Standard Schema can't. Intercept them BEFORE the
  // vendor dispatch; non-markers fall through to the per-vendor walker path.
  if (isMarkerSchema(schema)) {
    return schema[WIT_MARKER](compileSchema);
  }
  if (!isStandardSchema(schema)) {
    throw new Error(
      'Expected a Standard Schema value (a schema with a `~standard` property, e.g. from Zod / Valibot / ArkType / Effect Schema)',
    );
  }
  const vendor = schema['~standard'].vendor;
  const walker = walkers.get(vendor);
  if (!walker) {
    const known = registeredVendors();
    throw new Error(
      `No schema walker registered for vendor '${vendor}'. ` +
        (known.length ? `Registered vendors: ${known.join(', ')}.` : 'No walkers registered.') +
        ` Import the matching adapter (e.g. '@golemcloud/golem-ts-sdk' registers Zod).`,
    );
  }
  return walker(schema, compileSchema);
}
