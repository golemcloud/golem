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
  | { kind: 'boolean'; name?: string }
  | { kind: 'number'; name?: string }
  | { kind: 'string'; name?: string }
  | { kind: 'bigint'; name?: string }
  | { kind: 'null'; name?: string }
  | { kind: 'undefined'; name?: string }
  | { kind: 'void'; name?: string }
  | { kind: 'array'; name?: string; element: LiteTypeJSON }
  | { kind: 'tuple'; name?: string; elements: LiteTypeJSON[] }
  | { kind: 'union'; name?: string; types: LiteTypeJSON[] }
  | { kind: 'literal'; name: string }
  | {
      kind: 'object';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
    }
  | {
      kind: 'class';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
    }
  | {
      kind: 'interface';
      name?: string;
      properties: Array<{
        name: string;
        type: LiteTypeJSON;
        optional?: boolean;
      }>;
    }
  | {
      kind: 'promise';
      name?: string;
      element: LiteTypeJSON;
    }
  | {
      kind: 'alias';
      name: string;
      target: LiteTypeJSON;
    }
  | {
      kind: 'map';
      name?: string;
      typeArgs?: LiteTypeJSON[];
    }
  | { kind: 'others'; name?: string };
