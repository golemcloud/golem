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

// Assembles the v2 `AgentType` from the schema-native enriched representation,
// mirroring the Rust SDK `agentic::extended_agent_type`. All constructor /
// method / config schema roots are encoded into a single per-agent
// `SchemaGraph` (type-node pool) via one shared `GraphEncoder`, returning a
// `type-node-index` per root.

import {
  AgentConfigDeclaration,
  AgentConfigSource,
  AgentConstructor,
  AgentDependency,
  AgentMethod,
  AgentMode,
  AgentType,
  HttpEndpointDetails,
  HttpMountDetails,
  InputSchema,
  NamedField,
  OutputSchema,
  ReadOnlyConfig,
  Snapshotting,
} from 'golem:agent/common@2.0.0';
import {
  emptyMetadata,
  GraphEncoder,
  mergeGraphDefs,
  SchemaGraph,
  t,
  TypeId,
  SchemaTypeDef,
} from '../schema-model';
import { ConstructorArg } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { RuntimeOutput, RuntimeParam, RuntimeTypeInfo } from '../typeInfoInternal';
import { resolvedGraphToSchemaType } from '../mapping/types/schemaType';
import { mapTsTypeToResolvedGraph } from '../mapping/types/resolvedMapper';
import { TypeScope } from '../mapping/types/scope';
import {
  multimodalSchemaType,
  unstructuredBinarySchemaType,
  unstructuredTextSchemaType,
} from './rich';

export interface EnrichedConstructor {
  name?: string;
  description: string;
  promptHint?: string;
  params: RuntimeParam[];
}

export interface EnrichedMethod {
  name: string;
  description: string;
  promptHint?: string;
  httpEndpoint: HttpEndpointDetails[];
  readOnly?: ReadOnlyConfig;
  params: RuntimeParam[];
  output: RuntimeOutput;
}

export interface EnrichedConfig {
  source: AgentConfigSource;
  path: string[];
  valueGraph: SchemaGraph;
}

export interface AgentTypeInput {
  typeName: string;
  description: string;
  sourceLanguage: string;
  constructor: EnrichedConstructor;
  methods: EnrichedMethod[];
  dependencies: AgentDependency[];
  mode: AgentMode;
  httpMount?: HttpMountDetails;
  snapshotting: Snapshotting;
  config: EnrichedConfig[];
}

/**
 * Resolve the `config` constructor parameters into agent-level config
 * declarations. Each config property's value type is mapped to its own
 * `SchemaGraph`, later encoded into the shared agent schema graph.
 */
export function resolveAgentConfig(
  constructorParameters: readonly ConstructorArg[],
): Either.Either<EnrichedConfig[], string> {
  const entries: EnrichedConfig[] = [];

  for (const param of constructorParameters) {
    if (param.type.kind !== 'config') continue;

    for (const prop of param.type.properties) {
      const scope = TypeScope.object(param.name, prop.path.at(-1)!, prop.type.optional);
      const graphEither = mapTsTypeToResolvedGraph(prop.type, scope);
      if (Either.isLeft(graphEither)) {
        return Either.left(
          `parameter \`${param.name}\`, config property \`${prop.path.join('.')}\`: ${graphEither.val}`,
        );
      }

      entries.push({
        source: prop.secret ? 'secret' : 'local',
        path: prop.path,
        valueGraph: resolvedGraphToSchemaType(graphEither.val).graph,
      });
    }
  }

  return Either.right(entries);
}

/**
 * Project a single runtime parameter / output type into a self-contained
 * `SchemaGraph` (defs + root). Auto-injected (`principal`) and `config`
 * parameters have no value schema and must not reach this function.
 */
export function runtimeTypeInfoToSchemaGraph(type: RuntimeTypeInfo): SchemaGraph {
  switch (type.tag) {
    case 'schema':
      return resolvedGraphToSchemaType(type.graph).graph;
    case 'unstructured-text':
      return { defs: new Map(), root: unstructuredTextSchemaType(type.languages) };
    case 'unstructured-binary':
      return { defs: new Map(), root: unstructuredBinarySchemaType(type.mimeTypes) };
    case 'multimodal': {
      const caseGraphs = type.cases.map((c) => ({
        name: c.name,
        graph: runtimeTypeInfoToSchemaGraph(c.type),
      }));
      const defs = mergeGraphDefs(caseGraphs.map((c) => c.graph));
      const root = multimodalSchemaType(
        caseGraphs.map((c) => ({ name: c.name, root: c.graph.root })),
      );
      return { defs, root };
    }
    case 'principal':
      throw new Error("Internal error: 'principal' parameter has no value schema");
    case 'config':
      throw new Error("Internal error: 'config' parameter has no value schema");
  }
}

/** Collect every per-root graph that contributes definitions to the agent graph. */
function collectSchemaGraphs(input: AgentTypeInput): SchemaGraph[] {
  const graphs: SchemaGraph[] = [];

  const collectInputParams = (params: RuntimeParam[]) => {
    for (const param of params) {
      if (param.type.tag === 'principal' || param.type.tag === 'config') continue;
      graphs.push(runtimeTypeInfoToSchemaGraph(param.type));
    }
  };

  collectInputParams(input.constructor.params);
  for (const method of input.methods) {
    collectInputParams(method.params);
    if (method.output.tag === 'single') {
      graphs.push(runtimeTypeInfoToSchemaGraph(method.output.type));
    }
  }
  for (const config of input.config) {
    graphs.push(config.valueGraph);
  }

  return graphs;
}

function encodeInputSchema(encoder: GraphEncoder, params: RuntimeParam[]): InputSchema {
  const fields: NamedField[] = [];

  for (const param of params) {
    if (param.type.tag === 'config') continue;

    if (param.type.tag === 'principal') {
      fields.push({
        name: param.name,
        source: { tag: 'auto-injected', val: 'principal' },
        schema: encoder.encodeType(t.record([])),
        metadata: emptyMetadata(),
      });
    } else {
      const graph = runtimeTypeInfoToSchemaGraph(param.type);
      fields.push({
        name: param.name,
        source: { tag: 'user-supplied' },
        schema: encoder.encodeType(graph.root),
        metadata: emptyMetadata(),
      });
    }
  }

  return { tag: 'parameters', val: fields };
}

function encodeOutputSchema(encoder: GraphEncoder, output: RuntimeOutput): OutputSchema {
  if (output.tag === 'unit') {
    return { tag: 'unit' };
  }
  const graph = runtimeTypeInfoToSchemaGraph(output.type);
  return { tag: 'single', val: encoder.encodeType(graph.root) };
}

export function buildAgentType(input: AgentTypeInput): AgentType {
  const mergedDefs: Map<TypeId, SchemaTypeDef> = mergeGraphDefs(collectSchemaGraphs(input));
  const encoder = new GraphEncoder(mergedDefs);

  const constructorInput = encodeInputSchema(encoder, input.constructor.params);

  const methods: AgentMethod[] = input.methods.map((method) => ({
    name: method.name,
    description: method.description,
    promptHint: method.promptHint,
    httpEndpoint: method.httpEndpoint,
    readOnly: method.readOnly,
    inputSchema: encodeInputSchema(encoder, method.params),
    outputSchema: encodeOutputSchema(encoder, method.output),
  }));

  const config: AgentConfigDeclaration[] = input.config.map((c) => ({
    source: c.source,
    path: c.path,
    valueType: encoder.encodeType(c.valueGraph.root),
  }));

  const constructor: AgentConstructor = {
    name: input.constructor.name,
    description: input.constructor.description,
    promptHint: input.constructor.promptHint,
    inputSchema: constructorInput,
  };

  return {
    typeName: input.typeName,
    description: input.description,
    sourceLanguage: input.sourceLanguage,
    schema: encoder.finish(),
    constructor,
    methods,
    dependencies: input.dependencies,
    mode: input.mode,
    httpMount: input.httpMount,
    snapshotting: input.snapshotting,
    config,
  };
}
