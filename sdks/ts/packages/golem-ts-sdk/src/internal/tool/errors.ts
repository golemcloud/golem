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

export type ToolBuildErrorCode =
  | 'command-tree-cycle'
  | 'duplicate-command-parent'
  | 'invalid-identifier'
  | 'invalid-metadata-value'
  | 'subtree-root-name-mismatch'
  | 'duplicate-name'
  | 'duplicate-short'
  | 'unresolved-type-ref'
  | 'ill-formed-schema'
  | 'schema-conflict'
  | 'default-type-mismatch'
  | 'value-is-type-mismatch'
  | 'repeatable-map-type-not-map'
  | 'unresolved-default-formatter'
  | 'verbatim-without-separator'
  | 'variant-in-input-position'
  | 'command-not-found'
  | 'unresolved-constraint-ref'
  | 'unresolved-value-is-literal'
  | 'invalid-tail-occurrence-bounds'
  | 'required-positional-after-optional';

export class ToolBuildError extends Error {
  constructor(
    readonly code: ToolBuildErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'ToolBuildError';
  }
}

export function toolBuildError(code: ToolBuildErrorCode, message: string): never {
  throw new ToolBuildError(code, message);
}
