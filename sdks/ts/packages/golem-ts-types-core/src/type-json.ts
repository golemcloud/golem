// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

export type LiteTypeJSON =
  | { kind: 'boolean'; name?: string; optional: boolean }
  | { kind: 'number'; name?: string; optional: boolean }
  | { kind: 'string'; name?: string; optional: boolean }
  | { kind: 'bigint'; name?: string; optional: boolean }
  | { kind: 'null'; name?: string; optional: boolean }
  | { kind: 'undefined'; name?: string; optional: boolean }
  | { kind: 'void'; name?: string; optional: boolean }
  | { kind: 'array'; name?: string; element: LiteTypeJSON; optional: boolean }
  | {
      kind: 'tuple';
      name?: string;
      elements: LiteTypeJSON[];
      optional: boolean;
    }
  | {
      kind: 'union';
      name?: string;
      types: LiteTypeJSON[];
      typeParams: LiteTypeJSON[];
      optional: boolean;
      originalTypeName: string | undefined;
    }
  | { kind: 'literal'; name?: string; literalValue?: string; optional: boolean }
  | {
      kind: 'object';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
      typeParams: LiteTypeJSON[];
      optional: boolean;
    }
  | {
      kind: 'class';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
      optional: boolean;
    }
  | {
      kind: 'interface';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
      typeParams: LiteTypeJSON[];
      optional: boolean;
    }
  | {
      kind: 'promise';
      name?: string;
      element: LiteTypeJSON;
      optional: boolean;
    }
  | {
      kind: 'alias';
      name: string;
      target: LiteTypeJSON;
      optional: boolean;
    }
  | {
      kind: 'map';
      name?: string;
      typeArgs?: LiteTypeJSON[];
      optional: boolean;
    }
  | { kind: 'others'; name?: string; optional: boolean; recursive: boolean }
  | {
      kind: 'config';
      name?: string;
      optional: boolean;
      properties: { path: string[]; secret: boolean; type: LiteTypeJSON }[];
    }
  | {
      kind: 'unresolved-type';
      name?: string;
      optional: boolean;
      text: string;
      error: string;
    };
