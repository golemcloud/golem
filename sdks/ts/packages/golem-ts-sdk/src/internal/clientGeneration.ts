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

import { ClassMetadata, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import * as wasmRpc from 'golem:rpc/types@0.2.2';
import * as WitValue from './mapping/values/WitValue';
import * as Either from '../newTypes/either';
import { getAgentType, makeAgentId, RegisteredAgentType, Uuid } from 'golem:agent/host';
import { AgentClassName } from '../agentClassName';
import {
  AgentType,
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import * as Value from './mapping/values/Value';
import { BaseAgent, RemoteMethod } from '../baseAgent';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import { deserialize } from './mapping/values/deserializer';
import {
  serializeBinaryReferenceTsValue,
  serializeDefaultTsValue,
  serializeTextReferenceTsValue,
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
} from './mapping/values/serializer';
import { TypeInfoInternal } from './typeInfoInternal';
import {
  createSingleElementTupleDataValue,
  deserializeDataValue,
  ParameterDetail,
  serializeToDataValue,
} from './mapping/values/dataValue';
import { randomUuid } from '../host/hostapi';
import { convertAgentMethodNameToKebab } from './mapping/types/stringFormat';
import { AgentId } from '../agentId';
import * as util from 'node:util';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getRemoteClient<T extends new (...args: any[]) => BaseAgent>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: unknown[]) => {
    const instance = Object.create(ctor.prototype);

    const witAgentId = shared.constructAgentId(args);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, witAgentId));
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getPhantomRemoteClient<
  T extends new (phantomId: Uuid, ...args: any[]) => BaseAgent,
>(agentClassName: AgentClassName, agentType: AgentType, ctor: T) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }

  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (finalPhantomId: Uuid, ...args: unknown[]) => {
    const instance = Object.create(ctor.prototype);

    const witAgentId = shared.constructAgentId(args, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, witAgentId));
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function getNewPhantomRemoteClient<T extends new (...args: any[]) => BaseAgent>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: unknown[]) => {
    const instance = Object.create(ctor.prototype);

    const finalPhantomId = randomUuid();
    const witAgentId = shared.constructAgentId(args, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, witAgentId));
  };
}

type CachedParamInfo = {
  name: string;
  type: TypeInfoInternal;
};

type CachedMethodInfo = {
  name: string;
  kebabName: string;
  witFunctionName: string;
  params: CachedParamInfo[];
  returnType: TypeInfoInternal;
};

class WasmRpxProxyHandlerShared {
  readonly metadata: ClassMetadata;
  readonly agentClassName: AgentClassName;
  readonly agentType: AgentType;

  cachedRegisteredAgentType?: RegisteredAgentType = undefined;
  readonly constructorParamTypes: TypeInfoInternal[];
  readonly cachedMethodInfo: Map<string, CachedMethodInfo> = new Map();

  constructor(metadata: ClassMetadata, agentClassName: AgentClassName, agentType: AgentType) {
    this.metadata = metadata;
    this.agentClassName = agentClassName;
    this.agentType = agentType;

    const constructorParamMeta =
      AgentConstructorParamRegistry.get(agentClassName.value) ?? new Map();

    this.constructorParamTypes = [];
    for (const arg of metadata.constructorArgs) {
      const typeInfo = constructorParamMeta.get(arg.name)?.typeInfo;
      if (!typeInfo) {
        throw new Error(
          `No type information found for constructor parameter ${arg.name} in agent class ${agentClassName.value}`,
        );
      }
      this.constructorParamTypes.push(typeInfo);
    }
  }

  constructAgentId(args: unknown[], phantomId?: Uuid): wasmRpc.AgentId {
    const registeredAgentType = this.getRegisteredAgentType();

    if (args.length === 1 && this.constructorParamTypes[0].tag === 'multimodal') {
      const dataValueEither = serializeToDataValue(args[0], this.constructorParamTypes[0]);

      if (Either.isLeft(dataValueEither)) {
        throw new Error(
          `Failed to serialize multimodal constructor argument: ${dataValueEither.val}. Input is ${util.format(args)}`,
        );
      }

      const agentId = makeAgentId(this.agentClassName.value, dataValueEither.val, phantomId);

      return {
        componentId: registeredAgentType.implementedBy,
        agentId: agentId,
      };
    }

    const elementValues: ElementValue[] = [];
    for (const [index, arg] of args.entries()) {
      const typeInfoInternal = this.constructorParamTypes[index];

      switch (typeInfoInternal.tag) {
        case 'analysed':
          const witValue = Either.getOrThrowWith(
            WitValue.fromTsValueDefault(arg, typeInfoInternal.val),
            (err) => new Error(`Failed to encode constructor parameter ${arg}: ${err}`),
          );
          const elementValue: ElementValue = {
            tag: 'component-model',
            val: witValue,
          };
          elementValues.push(elementValue);
          break;
        case 'unstructured-text': {
          const textReference: TextReference = serializeTsValueToTextReference(arg);

          const elementValue: ElementValue = {
            tag: 'unstructured-text',
            val: textReference,
          };

          elementValues.push(elementValue);
          break;
        }
        case 'unstructured-binary':
          const binaryReference: BinaryReference = serializeTsValueToBinaryReference(arg);

          const elementValueBinary: ElementValue = {
            tag: 'unstructured-binary',
            val: binaryReference,
          };

          elementValues.push(elementValueBinary);
          break;
        case 'multimodal':
          throw new Error('Multimodal constructor parameters are not supported in remote calls');
      }
    }

    const constructorDataValue: DataValue = {
      tag: 'tuple',
      val: elementValues,
    };

    const agentId = makeAgentId(this.agentClassName.value, constructorDataValue, phantomId);

    return {
      componentId: registeredAgentType.implementedBy,
      agentId: agentId,
    };
  }

  getMethodInfo(methodName: string): CachedMethodInfo {
    const cachedInfo = this.cachedMethodInfo.get(methodName);
    if (cachedInfo) {
      return cachedInfo;
    } else {
      const methodSignature = this.metadata.methods.get(methodName);
      const methodParams = methodSignature?.methodParams;

      if (!methodParams) {
        throw new Error(
          `Unresolved method ${methodName} in RPC call. Make sure this method exists and is not private/protected`,
        );
      }

      const paramNames = Array.from(methodParams.keys());

      const paramTypeMap =
        AgentMethodParamRegistry.get(this.agentClassName.value)?.get(methodName) ?? new Map();

      const params = [];
      for (const paramName of paramNames) {
        const typeInfo = paramTypeMap.get(paramName)?.typeInfo;

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for parameter ${paramNames} in method ${methodName} in agent class ${this.agentClassName.value}`,
          );
        }

        params.push({ name: paramName, type: typeInfo });
      }

      const kebabName = convertAgentMethodNameToKebab(methodName);
      const witFunctionName = `${this.agentClassName.asWit}.{${kebabName}}`;
      const returnType = AgentMethodRegistry.getReturnType(this.agentClassName.value, methodName);

      if (!returnType) {
        throw new Error(
          `Return type of method ${methodName} in agent class ${this.agentClassName.value} is not supported in remote calls`,
        );
      }

      const cachedInfo = {
        name: methodName,
        kebabName,
        witFunctionName,
        params,
        returnType,
      };
      this.cachedMethodInfo.set(methodName, cachedInfo);
      return cachedInfo;
    }
  }

  private getRegisteredAgentType(): RegisteredAgentType {
    if (this.cachedRegisteredAgentType) {
      return this.cachedRegisteredAgentType;
    } else {
      const registeredAgentType = getAgentType(this.agentClassName.value);

      if (!registeredAgentType) {
        throw new Error(`There are no components implementing ${this.agentClassName.value}`);
      }

      this.cachedRegisteredAgentType = registeredAgentType;
      return registeredAgentType;
    }
  }
}

class WasmRpcProxyHandler implements ProxyHandler<Record<string, unknown>> {
  private readonly shared: WasmRpxProxyHandlerShared;
  private readonly agentId: AgentId;
  private readonly witAgentId: wasmRpc.AgentId;
  private readonly wasmRpc: wasmRpc.WasmRpc;

  private readonly methodProxyCache = new Map<string, RemoteMethod<unknown[], unknown>>();

  private readonly getIdMethod: () => AgentId = () => this.agentId;
  private readonly phantomIdMethod: () => Uuid | undefined = () => {
    const [_typeName, _params, phantomId] = this.agentId.parsed();
    return phantomId;
  };
  private readonly getAgentTypeMethod: () => AgentType = () => this.shared.agentType;

  constructor(shared: WasmRpxProxyHandlerShared, witAgentId: wasmRpc.AgentId) {
    this.shared = shared;
    this.agentId = new AgentId(witAgentId.agentId);
    this.witAgentId = witAgentId;

    this.wasmRpc = new wasmRpc.WasmRpc(witAgentId);
  }

  get(target: Record<string, unknown>, prop: string | symbol) {
    const val = target[prop.toString()];
    const propString = prop.toString();

    if (typeof val === 'function') {
      switch (propString) {
        case 'getId': {
          return this.getIdMethod;
        }
        case 'phantomId': {
          return this.phantomIdMethod;
        }
        case 'getAgentType': {
          return this.getAgentTypeMethod;
        }
        case 'loadSnapshot': {
          throw new Error('Cannot call loadSnapshot on a remote client');
        }
        case 'saveSnapshot': {
          throw new Error('Cannot call saveSnapshot on a remote client');
        }
        default:
          const methodProxy = this.methodProxyCache.get(propString);
          if (methodProxy) {
            return methodProxy;
          } else {
            const methodProxy = this.createMethodProxy(propString);
            this.methodProxyCache.set(propString, methodProxy);
            return methodProxy;
          }
      }
    }
    return undefined;
  }

  private createMethodProxy(prop: string): RemoteMethod<unknown[], unknown> {
    const methodInfo = this.shared.getMethodInfo(prop);
    const agentId = this.witAgentId;
    const wasmRpc = this.wasmRpc;

    async function invokeAndAwait(...fnArgs: unknown[]) {
      const parameterWitValues = serializeArgs(methodInfo.params, fnArgs);

      const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(
        methodInfo.witFunctionName,
        parameterWitValues,
      );

      const rpcResultPollable = rpcResultFuture.subscribe();

      await rpcResultPollable.promise();

      const rpcResult = rpcResultFuture.get();

      if (!rpcResult) {
        throw new Error(
          `RPC to remote agent failed. Failed to invoke ${methodInfo.name} in agent ${agentId.agentId}`,
        );
      }

      const rpcWitValue =
        rpcResult.tag === 'err'
          ? (() => {
              throw new Error(
                'Remote agent returned error result: ' + JSON.stringify(rpcResult.val),
              );
            })()
          : rpcResult.val;

      const rpcValueUnwrapped = unwrapResult(rpcWitValue);

      return deserializeRpcResult(rpcValueUnwrapped, methodInfo.returnType);
    }

    function invokeFireAndForget(...fnArgs: unknown[]) {
      const parameterWitValues = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.invoke(methodInfo.witFunctionName, parameterWitValues);
    }

    function invokeSchedule(ts: wasmRpc.Datetime, ...fnArgs: unknown[]) {
      const parameterWitValues = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.scheduleInvocation(ts, methodInfo.witFunctionName, parameterWitValues);
    }

    const methodFn = ((...args: unknown[]) => invokeAndAwait(...args)) as unknown as RemoteMethod<
      unknown[],
      unknown
    >;

    methodFn.trigger = (...args: unknown[]) => invokeFireAndForget(...args);
    methodFn.schedule = (ts: wasmRpc.Datetime, ...args: unknown[]) => invokeSchedule(ts, ...args);

    return methodFn;
  }
}

function convertToValue(
  arg: unknown,
  typeInfoInternal: TypeInfoInternal,
): Either.Either<Value.Value, string> {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      return serializeDefaultTsValue(arg, typeInfoInternal.val);

    case 'unstructured-text':
      return Either.right(serializeTextReferenceTsValue(arg));

    case 'unstructured-binary':
      return Either.right(serializeBinaryReferenceTsValue(arg));

    case 'principal':
      return Either.left(
        'Internal error: Value of `Principal` should not be serialized at any point during RPC call',
      );

    case 'multimodal': {
      const types = typeInfoInternal.types;

      const values: Value.Value[] = [];

      if (Array.isArray(arg)) {
        for (const elem of arg) {
          const multimodalElem = elem as { tag: string; val: unknown };
          const index = types.findIndex((paramDetail) => multimodalElem.tag === paramDetail.name);

          if (index === -1) {
            return Either.left(
              `Failed to serialize multimodal element: value is not matching any of the multimodal types`,
            );
          }

          const result = convertToValue(multimodalElem.val, types[index].type);

          if (Either.isLeft(result)) {
            return Either.left(`Failed to serialize multimodal element: ${result.val}`);
          }

          values.push({
            kind: 'variant',
            caseIdx: index,
            caseValue: result.val,
          });
        }
      } else {
        return Either.left(`Multimodal argument should be an array of values`);
      }

      return Either.right({
        kind: 'list',
        value: values,
      });
    }
  }
}

function serializeArgs(params: CachedParamInfo[], fnArgs: unknown[]): WitValue.WitValue[] {
  const result: WitValue.WitValue[] = [];
  for (const [index, fnArg] of fnArgs.entries()) {
    const param = params[index];
    const value = Either.getOrThrowWith(
      convertToValue(fnArg, param.type),
      (err) => new Error(`Failed to serialize arg ${param.name}: ${err}`),
    );
    const witValue = Value.toWitValue(value);
    result.push(witValue);
  }
  return result;
}

function unwrapResult(witValue: WitValue.WitValue): Value.Value {
  const value = Value.fromWitValue(witValue);

  return value.kind === 'tuple' && value.value.length > 0 ? value.value[0] : value;
}

function deserializeRpcResult<T = unknown>(
  rpcResult: Value.Value,
  typeInfoInternal: TypeInfoInternal,
): T {
  return _deserializeRpcResult(rpcResult, typeInfoInternal) as T;
}

function _deserializeRpcResult(
  rpcResult: Value.Value,
  typeInfoInternal: TypeInfoInternal,
): unknown {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      const dataValue = createSingleElementTupleDataValue({
        tag: 'component-model',
        val: Value.toWitValue(rpcResult),
      });

      return Either.getOrThrowWith(
        deserializeDataValue(
          dataValue,
          [
            {
              name: 'return-value',
              type: typeInfoInternal,
            },
            // Deserializing rpc result doesn't require principal context
            // i.e, return type of a method is never conceived to be 'Principal' anywhere in SDK,
            // but the type is normalized to be simple component-model type
          ],
          { tag: 'anonymous' },
        ),

        (err) => new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'unstructured-text':
      const textReference = convertValueToTextReference(rpcResult);

      const dataValueText = createSingleElementTupleDataValue({
        tag: 'unstructured-text',
        val: textReference,
      });

      return Either.getOrThrowWith(
        deserializeDataValue(
          dataValueText,
          [
            {
              name: 'return-value',
              type: typeInfoInternal,
            },
            // Deserializing rpc result doesn't require principal context,
            // In this case typeInfoInternal is 'unstructured-text', so Principal type cannot appear here
          ],
          { tag: 'anonymous' },
        ),
        (err) => new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'unstructured-binary':
      const binaryReference = convertValueToBinaryReference(rpcResult);

      const dataValueBinary = createSingleElementTupleDataValue({
        tag: 'unstructured-binary',
        val: binaryReference,
      });

      return Either.getOrThrowWith(
        deserializeDataValue(
          dataValueBinary,
          [
            {
              name: 'return-value',
              type: typeInfoInternal,
            },
            // Deserializing rpc result doesn't require principal context,
            // In this case typeInfoInternal is 'unstructured-binary', so Principal type cannot appear here
          ],
          { tag: 'anonymous' },
        ),
        (err) => new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'multimodal':
      const multimodalParamsInfo: ParameterDetail[] = typeInfoInternal.types;

      switch (rpcResult.kind) {
        // A multimodal value is always a list
        case 'list':
          const values = rpcResult.value;

          const nameAndElementValues: [string, ElementValue][] = values.map((value, idx) => {
            switch (value.kind) {
              case 'variant':
                const caseIdx = value.caseIdx;
                const paramDetail = multimodalParamsInfo[caseIdx];
                const caseValue = value.caseValue;

                if (!caseValue) {
                  throw new Error(`Missing case value in multimodal return value at index ${idx}`);
                }

                const elementValue = convertNonMultimodalValueToElementValue(
                  caseValue,
                  paramDetail.type,
                );

                return [paramDetail.name, elementValue];

              default:
                throw new Error(
                  `Invalid kind in multimodal return value at index ${idx}: expected variant, got ${value.kind}`,
                );
            }
          });

          const dataValue: DataValue = {
            tag: 'multimodal',
            val: nameAndElementValues,
          };

          return Either.getOrThrowWith(
            deserializeDataValue(
              dataValue,
              [
                {
                  name: 'return-value',
                  type: typeInfoInternal,
                },
              ], // Deserializing rpc result doesn't require principal context,
              // and multimodal cannot contain Principal type inside
              { tag: 'anonymous' },
            ),
            (err) => new Error(`Failed to deserialize multimodal return value: ${err}`),
          )[0];
      }
  }
}

function convertNonMultimodalValueToElementValue(
  rpcValueUnwrapped: Value.Value,
  returnTypeInfoInternal: TypeInfoInternal,
): ElementValue {
  switch (returnTypeInfoInternal.tag) {
    case 'analysed':
      return {
        tag: 'component-model',
        val: Value.toWitValue(rpcValueUnwrapped),
      };

    case 'unstructured-text':
      const textReference = convertValueToTextReference(rpcValueUnwrapped);

      return {
        tag: 'unstructured-text',
        val: textReference,
      };

    case 'principal':
      throw new Error(`Internal error: Value of 'Principal' should not appear in RPC calls`);

    case 'unstructured-binary':
      const binaryReference = convertValueToBinaryReference(rpcValueUnwrapped);

      return {
        tag: 'unstructured-binary',
        val: binaryReference,
      };

    case 'multimodal':
      // DataValue::Multimodal cannot encode recursive multimodals
      throw new Error(`Nested multimodal values are not supported`);
  }
}

function convertValueToTextReference(value: Value.Value): TextReference {
  switch (value.kind) {
    case 'variant':
      const idx = value.caseIdx;
      switch (idx) {
        case 0:
          // url
          const urlValue = value.caseValue;

          if (!urlValue) {
            throw new Error(`Unable to extract URL from value`);
          }

          switch (urlValue.kind) {
            case 'string':
              return {
                tag: 'url',
                val: urlValue.value,
              };

            default:
              throw new Error(`Invalid URL value type in value: ${urlValue.kind}`);
          }

        case 1:
          // inline
          const inlineValue = value.caseValue;

          if (!inlineValue) {
            throw new Error(`Unable to extract inline text from value`);
          }

          switch (inlineValue.kind) {
            case 'record':
              const record = inlineValue.value;
              const data = record[0];
              const languageCodeField = record.length > 1 ? record[1] : undefined;

              switch (data.kind) {
                case 'string':
                  const textData = data.value;

                  // The languageCode field doesn't exist at all
                  if (!languageCodeField) {
                    return {
                      tag: 'inline',
                      val: {
                        data: textData,
                      },
                    };
                  }

                  switch (languageCodeField.kind) {
                    case 'option':
                      const langCodeOpt = languageCodeField.value;

                      // The languageCode field exists; however, it's None
                      if (!langCodeOpt) {
                        return {
                          tag: 'inline',
                          val: {
                            data: textData,
                          },
                        };
                      }

                      switch (langCodeOpt.kind) {
                        case 'string':
                          const languageCodeStrOpt = langCodeOpt.value;
                          return {
                            tag: 'inline',
                            val: {
                              data: textData,
                              textType: { languageCode: languageCodeStrOpt },
                            },
                          };

                        default:
                          throw new Error(
                            `Invalid inline text language code option type: expected string, found ${JSON.stringify(langCodeOpt)}`,
                          );
                      }

                    default:
                      throw new Error(
                        `Invalid inline text language code type: expected string, found ${languageCodeField.kind}`,
                      );
                  }

                default:
                  throw new Error(`Invalid inline text data type: expected string`);
              }
            default:
              throw new Error(`Invalid inline text value type in value: ${inlineValue.kind}`);
          }
      }
  }

  throw new Error(`Unable to convert value to TextReference`);
}

function convertValueToBinaryReference(value: Value.Value): BinaryReference {
  switch (value.kind) {
    case 'variant':
      const idx = value.caseIdx;
      switch (idx) {
        case 0:
          // url
          const urlValue = value.caseValue;

          if (!urlValue) {
            throw new Error(`Unable to extract URL from value`);
          }

          switch (urlValue.kind) {
            case 'string':
              return {
                tag: 'url',
                val: urlValue.value,
              };

            default:
              throw new Error(`Invalid URL value type in value: ${urlValue.kind}`);
          }

        case 1:
          // inline
          const inlineValue = value.caseValue;

          if (!inlineValue) {
            throw new Error(`Unable to extract inline binary from value`);
          }

          switch (inlineValue.kind) {
            case 'record':
              const values = inlineValue.value;

              const data = values[0];

              const uint8Array: Uint8Array = deserialize(data, {
                kind: 'list',
                value: {
                  name: undefined,
                  owner: undefined,
                  inner: { kind: 'u8' },
                },
                typedArray: 'u8',
                mapType: undefined,
              }) as Uint8Array;

              const mimeType = values[1];

              if (!mimeType) {
                throw new Error(`Unable to extract mime type from value`);
              }

              switch (mimeType.kind) {
                case 'string':
                  return {
                    tag: 'inline',
                    val: {
                      data: uint8Array,
                      binaryType: {
                        mimeType: mimeType.value,
                      },
                    },
                  };
                default:
                  throw new Error(`Invalid inline binary mime type type: expected string`);
              }

            default:
              throw new Error(`Invalid inline binary value type in value: ${inlineValue.kind}`);
          }
      }
  }

  throw new Error(`Unable to convert value to BinaryReference`);
}
