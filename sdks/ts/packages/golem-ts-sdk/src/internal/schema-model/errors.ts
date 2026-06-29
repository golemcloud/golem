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

// Structured errors for the schema model and its WIT codecs. These never
// silently drop information: every failure path raises one of these with a
// descriptive message so callers can surface a precise diagnostic.

/** Raised when encoding an in-memory schema/value into its flat WIT carrier fails. */
export class SchemaEncodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SchemaEncodeError';
  }
}

/** Raised when decoding a flat WIT carrier into the in-memory model fails. */
export class SchemaDecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SchemaDecodeError';
  }
}

/**
 * Raised when merging per-root schema graphs encounters two definitions that
 * share the same `type-id` but have structurally different bodies.
 */
export class SchemaConflictError extends Error {
  constructor(
    public readonly typeId: string,
    message?: string,
  ) {
    super(message ?? `conflicting definitions for type id '${typeId}'`);
    this.name = 'SchemaConflictError';
  }
}
