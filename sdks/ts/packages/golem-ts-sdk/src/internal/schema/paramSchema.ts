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

import { DataSchema, ElementSchema } from 'golem:agent/common@1.5.0';

// Collection of parameter schemas for agent constructor or method.
// It can contain both principal and component model parameters.
// Mainly to differentiate between them and generate DataSchema for component model parameters only.
export class ParameterSchemaCollection {
  private parameterSchemas: ParameterSchema[] = [];

  addPrincipalParameter(name: string): void {
    this.parameterSchemas.push({ tag: 'principal', name });
  }

  addConfigParameter(name: string): void {
    this.parameterSchemas.push({ tag: 'config', name });
  }

  addComponentModelParameter(name: string, schema: ElementSchema): void {
    this.parameterSchemas.push({
      tag: 'component-model',
      name,
      schema,
    });
  }

  getDataSchema(): DataSchema {
    return getDataSchema(this.parameterSchemas);
  }
}

export type ParameterSchema =
  | { tag: 'principal'; name: string }
  | { tag: 'config'; name: string }
  | { tag: 'component-model'; name: string; schema: ElementSchema };

// Remove principal parameters
function getDataSchema(parameterSchemaCollection: ParameterSchema[]): DataSchema {
  let nameAndSchema: [string, ElementSchema][] = [];

  for (const paramSchema of parameterSchemaCollection) {
    switch (paramSchema.tag) {
      case 'config':
        break;
      case 'principal':
        break;
      case 'component-model':
        nameAndSchema.push([paramSchema.name, paramSchema.schema]);
        break;
    }
  }
  return {
    tag: 'tuple',
    val: nameAndSchema,
  };
}
