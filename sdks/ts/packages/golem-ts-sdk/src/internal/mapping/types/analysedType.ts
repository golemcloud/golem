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

import { convertOptionalTypeNameToKebab } from './stringFormat';
import { TaggedTypeMetadata } from './taggedUnion';

export interface NameTypePair {
  name: string;
  typ: AnalysedType;
}

export interface NameOptionTypePair {
  name: string;
  typ?: AnalysedType;
}

export type TypedArray =
  | 'u8'
  | 'u16'
  | 'u32'
  | 'big-u64'
  | 'i8'
  | 'i16'
  | 'i32'
  | 'big-i64'
  | 'f32'
  | 'f64';

export type EmptyType = 'null' | 'void' | 'undefined' | 'question-mark';

export type CustomOrInbuilt =
  | {
      tag: 'custom';
      okValueName: string | undefined;
      errValueName: string | undefined;
    }
  | {
      tag: 'inbuilt';
      okEmptyType: EmptyType | undefined;
      errEmptyType: EmptyType | undefined;
    };

// This is similar to internal analyzed-type in wasm-rpc (golem)
// while having extra information useful for WIT -> WIT type and value mapping
export type AnalysedType =
  | { kind: 'variant'; value: TypeVariant; taggedTypes: TaggedTypeMetadata[] }
  | { kind: 'result'; value: TypeResult; resultType: CustomOrInbuilt }
  | { kind: 'option'; value: TypeOption; emptyType: EmptyType }
  | { kind: 'enum'; value: TypeEnum }
  | { kind: 'flags'; value: TypeFlags }
  | { kind: 'record'; value: TypeRecord }
  | { kind: 'tuple'; value: TypeTuple; emptyType: EmptyType | undefined }
  | {
      kind: 'list';
      value: TypeList;
      typedArray: TypedArray | undefined;
      mapType: { keyType: AnalysedType; valueType: AnalysedType } | undefined;
    }
  | { kind: 'string' }
  | { kind: 'chr' }
  | { kind: 'f64' }
  | { kind: 'f32' }
  | { kind: 'u64'; isBigInt: boolean }
  | { kind: 's64'; isBigInt: boolean }
  | { kind: 'u32' }
  | { kind: 's32' }
  | { kind: 'u16' }
  | { kind: 's16' }
  | { kind: 'u8' }
  | { kind: 's8' }
  | { kind: 'bool' }
  | { kind: 'handle'; value: TypeHandle };

export function getNameFromAnalysedType(typ: AnalysedType): string | undefined {
  switch (typ.kind) {
    case 'string':
      return undefined;
    case 'chr':
      return undefined;
    case 'f64':
      return undefined;
    case 'f32':
      return undefined;
    case 'u64':
      return undefined;
    case 's64':
      return undefined;
    case 'u32':
      return undefined;
    case 's32':
      return undefined;
    case 'u16':
      return undefined;
    case 's16':
      return undefined;
    case 'u8':
      return undefined;
    case 's8':
      return undefined;
    case 'bool':
      return undefined;
    case 'handle':
      return typ.value.name;
    case 'variant':
      return typ.value.name;
    case 'result':
      return typ.value.name;
    case 'option':
      return typ.value.name;
    case 'enum':
      return typ.value.name;
    case 'flags':
      return typ.value.name;
    case 'record':
      return typ.value.name;
    case 'tuple':
      return typ.value.name;
    case 'list':
      return typ.value.name;
  }
}

export function getOwnerFromAnalysedType(typ: AnalysedType): string | undefined {
  switch (typ.kind) {
    case 'variant':
      return typ.value.owner;
    case 'result':
      return typ.value.owner;
    case 'option':
      return typ.value.owner;
    case 'enum':
      return typ.value.owner;
    case 'flags':
      return typ.value.owner;
    case 'record':
      return typ.value.owner;
    case 'tuple':
      return typ.value.owner;
    case 'list':
      return typ.value.owner;
    case 'handle':
      return typ.value.owner;
    default:
      return undefined;
  }
}

export interface TypeResult {
  name: string | undefined;
  owner: string | undefined;
  ok?: AnalysedType;
  err?: AnalysedType;
}

export interface TypeVariant {
  name: string | undefined;
  owner: string | undefined;
  cases: NameOptionTypePair[];
}

export interface TypeOption {
  name: string | undefined;
  owner: string | undefined;
  inner: AnalysedType;
}

export interface TypeEnum {
  name: string | undefined;
  owner: string | undefined;
  cases: string[];
}

export interface TypeFlags {
  name: string | undefined;
  owner: string | undefined;
  names: string[];
}

export interface TypeRecord {
  name: string | undefined;
  owner: string | undefined;
  fields: NameTypePair[];
}

export interface TypeTuple {
  name: string | undefined;
  owner: string | undefined;
  items: AnalysedType[];
}

export interface TypeList {
  name: string | undefined;
  owner: string | undefined;
  inner: AnalysedType;
}

export interface TypeHandle {
  name: string | undefined;
  owner: string | undefined;
  resourceId: AnalysedResourceId;
  mode: AnalysedResourceMode;
}

export type AnalysedResourceMode = 'owned' | 'borrowed';

export type AnalysedResourceId = number;

export function field(name: string, typ: AnalysedType): NameTypePair {
  return { name, typ };
}

export function case_(name: string, typ: AnalysedType): NameOptionTypePair {
  return { name, typ };
}

export function optCase(name: string, typ?: AnalysedType): NameOptionTypePair {
  return { name, typ };
}

export function unitCase(name: string): NameOptionTypePair {
  return { name };
}

export function bool(): AnalysedType {
  return { kind: 'bool' };
}

export function str(): AnalysedType {
  return { kind: 'string' };
}

export function chr(): AnalysedType {
  return { kind: 'chr' };
}

export function f64(): AnalysedType {
  return { kind: 'f64' };
}

export function f32(): AnalysedType {
  return { kind: 'f32' };
}

export function u64(isBigInt: boolean): AnalysedType {
  return { kind: 'u64', isBigInt };
}

export function s64(isBigInt: boolean): AnalysedType {
  return { kind: 's64', isBigInt };
}

export function u32(): AnalysedType {
  return { kind: 'u32' };
}

export function s32(): AnalysedType {
  return { kind: 's32' };
}

export function u16(): AnalysedType {
  return { kind: 'u16' };
}

export function s16(): AnalysedType {
  return { kind: 's16' };
}

export function u8(): AnalysedType {
  return { kind: 'u8' };
}

export function s8(): AnalysedType {
  return { kind: 's8' };
}

export function list(
  name: string | undefined,
  typedArrayKind: TypedArray | undefined,
  mapType: { keyType: AnalysedType; valueType: AnalysedType } | undefined,
  inner: AnalysedType,
): AnalysedType {
  return {
    kind: 'list',
    typedArray: typedArrayKind,
    mapType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      inner,
    },
  };
}

export function option(
  name: string | undefined,
  emptyType: EmptyType,
  inner: AnalysedType,
): AnalysedType {
  return {
    kind: 'option',
    emptyType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      inner,
    },
  };
}

export function tuple(
  name: string | undefined,
  emptyType: EmptyType | undefined,
  items: AnalysedType[],
): AnalysedType {
  return {
    kind: 'tuple',
    emptyType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      items,
    },
  };
}

export function record(name: string | undefined, fields: NameTypePair[]): AnalysedType {
  return {
    kind: 'record',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      fields,
    },
  };
}

export function flags(name: string | undefined, names: string[]): AnalysedType {
  return {
    kind: 'flags',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      names,
    },
  };
}

export function enum_(name: string | undefined, cases: string[]): AnalysedType {
  return {
    kind: 'enum',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      cases,
    },
  };
}

export function variant(
  name: string | undefined,
  taggedTypes: TaggedTypeMetadata[],
  cases: NameOptionTypePair[],
): AnalysedType {
  return {
    kind: 'variant',
    taggedTypes,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      cases,
    },
  };
}

export function result(
  name: string | undefined,
  resultType: CustomOrInbuilt,
  ok: AnalysedType | undefined,
  err: AnalysedType | undefined,
): AnalysedType {
  return {
    kind: 'result',
    resultType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      ok,
      err,
    },
  };
}

export function handle(
  name: string | undefined,
  resourceId: AnalysedResourceId,
  mode: AnalysedResourceMode,
): AnalysedType {
  return {
    kind: 'handle',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      resourceId,
      mode,
    },
  };
}
