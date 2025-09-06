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

import { Type } from './type-lite';
import { LiteTypeJSON } from './type-json';
import { buildTypeFromJSON } from './json-to-type';

type ClassNameString = string;
type MethodNameString = string;

export type MethodParams = Map<string, Type>;

export type ConstructorArg = { name: string; type: Type };

export type ClassMetadata = {
  constructorArgs: ConstructorArg[];
  methods: Map<
    MethodNameString,
    { methodParams: MethodParams; returnType: Type }
  >;
};

const Metadata = new Map<ClassNameString, ClassMetadata>();

export const TypeMetadata = {
  update(
    className: ClassNameString,
    constructorArgs: ConstructorArg[],
    methods: Map<
      MethodNameString,
      { methodParams: MethodParams; returnType: Type }
    >,
  ) {
    Metadata.set(className, { constructorArgs, methods });
  },

  get(className: string): ClassMetadata | undefined {
    return Metadata.get(className);
  },

  clearMetadata(): void {
    Metadata.clear();
    return;
  },

  getAll(): Map<ClassNameString, ClassMetadata> {
    return Metadata;
  },

  clearAll(): void {
    Metadata.clear();
  },

  // TODO: avoid any. Here any simply represents the json representation of Metadata
  // such that every Type is represented as LiteTypeJSON
  loadFromJson(json: any) {
    for (const [className, meta] of Object.entries(json)) {
      const constructorArgsJSON = (meta as any).constructorArgs as Array<{
        name: string;
        type: LiteTypeJSON;
      }>;

      const constructorArgs = constructorArgsJSON.map((arg) => ({
        name: arg.name,
        type: buildTypeFromJSON(arg.type),
      }));

      const methodsMap = new Map<
        string,
        { methodParams: Map<string, Type>; returnType: Type }
      >();

      for (const [methodName, methodMeta] of Object.entries(
        (meta as any).methods,
      )) {
        const methodParamsMap = new Map<string, Type>();
        for (const [paramName, paramJSON] of Object.entries(
          (methodMeta as any).methodParams,
        )) {
          methodParamsMap.set(
            paramName,
            buildTypeFromJSON(paramJSON as LiteTypeJSON),
          );
        }

        methodsMap.set(methodName, {
          methodParams: methodParamsMap,
          returnType: buildTypeFromJSON(
            (methodMeta as any).returnType as LiteTypeJSON,
          ),
        });
      }

      TypeMetadata.update(className, constructorArgs, methodsMap);
    }
  },
};
