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

import {
  Project,
  SourceFile,
  Type as TsMorphType,
  Symbol
} from 'ts-morph';

export interface WellKnown {
  type: TsMorphType;
  symbol: Symbol;
}

export interface WellKnownTypes {
  object: WellKnown,
  containers: {
    promise: WellKnown;
    map: WellKnown;
    typedArrays: Map<string, WellKnown>;
  };
}

export function createWellKnownTypes(project: Project): WellKnownTypes {
  const sf = project.createSourceFile(
    "__golem_well_known_types__.ts",
    `
      let _object!: Object;

      let _promise!: Promise<any>;
      let _map!: Map<any, any>;

      let _Float64Array!: Float64Array;
      let _Float32Array!: Float32Array;
      let _Int8Array!: Int8Array;
      let _Uint8Array!: Uint8Array;
      let _Int16Array!: Int16Array;
      let _Uint16Array!: Uint16Array;
      let _Int32Array!: Int32Array;
      let _Uint32Array!: Uint32Array;
      let _BigInt64Array!: BigInt64Array;
      let _BigUint64Array!: BigUint64Array;
    `,
    { overwrite: true }
  );

  const containers = {
    promise: getWellKnownFromVar(sf, "_promise"),
    map: getWellKnownFromVar(sf, "_map"),
    typedArrays: new Map<string, WellKnown>([
      ["Float64Array", getWellKnownFromVar(sf, "_Float64Array")],
      ["Float32Array", getWellKnownFromVar(sf, "_Float32Array")],
      ["Int8Array", getWellKnownFromVar(sf, "_Int8Array")],
      ["Uint8Array", getWellKnownFromVar(sf, "_Uint8Array")],
      ["Int16Array", getWellKnownFromVar(sf, "_Int16Array")],
      ["Uint16Array", getWellKnownFromVar(sf, "_Uint16Array")],
      ["Int32Array", getWellKnownFromVar(sf, "_Int32Array")],
      ["Uint32Array", getWellKnownFromVar(sf, "_Uint32Array")],
      ["BigInt64Array", getWellKnownFromVar(sf, "_BigInt64Array")],
      ["BigUint64Array", getWellKnownFromVar(sf, "_BigUint64Array")],
    ]),
  };

  return {
    object: getWellKnownFromVar(sf, "_object"),
    containers,
  };
}

function getWellKnownFromVar(
  sf: SourceFile,
  name: string,
): WellKnown {
  const type = sf.getVariableDeclarationOrThrow(name).getType();
  const symbol = type.getSymbol();

  if (!symbol) {
    throw new Error(`No symbol for ${name}`);
  }

  return { type, symbol };
}
