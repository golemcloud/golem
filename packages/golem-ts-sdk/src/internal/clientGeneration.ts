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
import { AgentId, Datetime, WasmRpc } from 'golem:rpc/types@0.2.2';
import * as WitValue from './mapping/values/WitValue';
import * as Option from '../newTypes/option';
import * as Either from '../newTypes/either';
import {
  getAgentType,
  makeAgentId,
  RegisteredAgentType,
  Uuid,
} from 'golem:agent/host';
import { AgentClassName } from '../newTypes/agentClassName';
import {
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import * as Value from './mapping/values/Value';
import { RemoteMethod } from '../baseAgent';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import { deserialize } from './mapping/values/deserializer';
import {
  matchesType,
  serializeBinaryReferenceTsValue,
  serializeDefaultTsValue,
  serializeTextReferenceTsValue,
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
} from './mapping/values/serializer';
import { TypeInfoInternal } from './registry/typeInfoInternal';
import {
  createSingleElementTupleDataValue,
  deserializeDataValue,
  ParameterDetail,
} from './mapping/values/dataValue';
import { randomUuid } from '../host/hostapi';
import { convertAgentMethodNameToKebab } from './mapping/types/stringFormat';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  ctor: T,
) {
  const metadataOpt = Option.fromNullable(TypeMetadata.get(ctor.name));
  if (Option.isNone(metadataOpt)) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const metadata = metadataOpt.val;
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName);

  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const agentId = shared.constructAgentId(args);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, agentId));
  };
}

export function getPhantomRemoteClient<
  T extends new (phantomId: Uuid | undefined, ...args: any[]) => any,
>(agentClassName: AgentClassName, ctor: T) {
  const metadataOpt = Option.fromNullable(TypeMetadata.get(ctor.name));
  if (Option.isNone(metadataOpt)) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const metadata = metadataOpt.val;
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName);

  return (phantomId: Uuid | undefined, ...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const finalPhantomId = phantomId ?? randomUuid();
    const agentId = shared.constructAgentId(args, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, agentId));
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

  cachedRegisteredAgentType?: RegisteredAgentType = undefined;
  readonly constructorParamTypes: TypeInfoInternal[];
  readonly cachedMethodInfo: Map<string, CachedMethodInfo> = new Map();

  constructor(metadata: ClassMetadata, agentClassName: AgentClassName) {
    this.metadata = metadata;
    this.agentClassName = agentClassName;

    const constructorParamMeta = AgentConstructorParamRegistry.get(
      agentClassName.value,
    );
    if (!constructorParamMeta) {
      throw new Error(
        `No constructor parameter metadata found for ${agentClassName.value}`,
      );
    }

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

  constructAgentId(args: any[], phantomId?: Uuid): AgentId {
    const registeredAgentType = this.getRegisteredAgentType();

    const elementValues: ElementValue[] = [];
    for (const [index, arg] of args.entries()) {
      const typeInfoInternal = this.constructorParamTypes[index];

      switch (typeInfoInternal.tag) {
        case 'analysed':
          const witValue = Either.getOrThrowWith(
            WitValue.fromTsValueDefault(arg, typeInfoInternal.val),
            (err) =>
              new Error(
                `Failed to encode constructor parameter ${arg}: ${err}`,
              ),
          );
          const elementValue: ElementValue = {
            tag: 'component-model',
            val: witValue,
          };
          elementValues.push(elementValue);
          break;
        case 'unstructured-text': {
          const textReference: TextReference =
            serializeTsValueToTextReference(arg);

          const elementValue: ElementValue = {
            tag: 'unstructured-text',
            val: textReference,
          };

          elementValues.push(elementValue);
          break;
        }
        case 'unstructured-binary':
          const binaryReference: BinaryReference =
            serializeTsValueToBinaryReference(arg);

          const elementValueBinary: ElementValue = {
            tag: 'unstructured-binary',
            val: binaryReference,
          };

          elementValues.push(elementValueBinary);
          break;
        case 'multimodal':
          throw new Error(
            'Multimodal constructor parameters are not supported in remote calls',
          );
      }
    }

    const constructorDataValue: DataValue = {
      tag: 'tuple',
      val: elementValues,
    };

    const agentId = makeAgentId(
      this.agentClassName.value,
      constructorDataValue,
      phantomId,
    );

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
        AgentMethodParamRegistry.get(this.agentClassName.value)?.get(
          methodName,
        ) ?? new Map();

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
      const returnType = AgentMethodRegistry.getReturnType(
        this.agentClassName.value,
        methodName,
      );

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
        throw new Error(
          `There are no components implementing ${this.agentClassName.value}`,
        );
      }

      this.cachedRegisteredAgentType = registeredAgentType;
      return registeredAgentType;
    }
  }
}

class WasmRpcProxyHandler implements ProxyHandler<any> {
  private readonly shared: WasmRpxProxyHandlerShared;
  private readonly agentId: AgentId;
  private readonly wasmRpc: WasmRpc;

  private readonly methodProxyCache = new Map<
    string,
    RemoteMethod<any[], any>
  >();

  constructor(shared: WasmRpxProxyHandlerShared, agentId: AgentId) {
    this.shared = shared;
    this.agentId = agentId;

    this.wasmRpc = new WasmRpc(agentId);
  }

  get(target: any, prop: string | symbol) {
    const val = target[prop];
    const propString = prop.toString();

    if (typeof val === 'function') {
      const methodProxy = this.methodProxyCache.get(propString);
      if (methodProxy) {
        return methodProxy;
      } else {
        const methodProxy = this.createMethodProxy(propString);
        this.methodProxyCache.set(propString, methodProxy);
        return methodProxy;
      }
    }
    return undefined;
  }

  private createMethodProxy(prop: string): RemoteMethod<any[], any> {
    const methodInfo = this.shared.getMethodInfo(prop);
    const agentId = this.agentId;
    const wasmRpc = this.wasmRpc;

    async function invokeAndAwait(...fnArgs: any[]) {
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
          `Failed to invoke ${methodInfo.name} in agent ${agentId.agentId}`,
        );
      }

      const rpcWitValue =
        rpcResult.tag === 'err'
          ? (() => {
              throw new Error(
                'Failed to invoke: ' + JSON.stringify(rpcResult.val),
              );
            })()
          : rpcResult.val;

      const rpcValueUnwrapped = unwrapResult(rpcWitValue);

      return deserializeRpcResult(rpcValueUnwrapped, methodInfo.returnType);
    }

    function invokeFireAndForget(...fnArgs: any[]) {
      const parameterWitValues = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.invoke(methodInfo.witFunctionName, parameterWitValues);
    }

    function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
      const parameterWitValues = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.scheduleInvocation(
        ts,
        methodInfo.witFunctionName,
        parameterWitValues,
      );
    }

    const methodFn: any = (...args: any[]) => invokeAndAwait(...args);

    methodFn.trigger = (...args: any[]) => invokeFireAndForget(...args);
    methodFn.schedule = (ts: Datetime, ...args: any[]) =>
      invokeSchedule(ts, ...args);

    return methodFn as RemoteMethod<any[], any>;
  }
}

function convertToValue(
  arg: any,
  typeInfoInternal: TypeInfoInternal,
): Either.Either<Value.Value, string> {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      return serializeDefaultTsValue(arg, typeInfoInternal.val);

    case 'unstructured-text':
      return Either.right(serializeTextReferenceTsValue(arg));

    case 'unstructured-binary':
      return Either.right(serializeBinaryReferenceTsValue(arg));

    case 'multimodal':
      const types = typeInfoInternal.types;

      const values: Value.Value[] = [];

      if (Array.isArray(arg)) {
        // Pre-compute type matchers to avoid redundant type checking per element
        const typeMatchers = types.map((paramDetail) => {
          const type = paramDetail.type;
          switch (type.tag) {
            case 'analysed':
              return (elem: any) => matchesType(elem, type.val);

            case 'unstructured-binary':
              return (elem: any) => {
                const isObjectBinary =
                  typeof elem === 'object' && elem !== null;
                const keysBinary = Object.keys(elem);
                return (
                  isObjectBinary &&
                  keysBinary.includes('tag') &&
                  (elem['tag'] === 'url' || elem['tag'] === 'inline')
                );
              };

            case 'unstructured-text':
              return (elem: any) => {
                const isObject = typeof elem === 'object' && elem !== null;
                const keys = Object.keys(elem);
                return (
                  isObject &&
                  keys.includes('tag') &&
                  (elem['tag'] === 'url' || elem['tag'] === 'inline')
                );
              };

            case 'multimodal':
              throw new Error(`Nested multimodal types are not supported`);
          }
        });

        for (const elem of arg) {
          const index = typeMatchers.findIndex((matcher) => matcher(elem));

          const result = convertToValue(arg[index], types[index].type);

          if (Either.isLeft(result)) {
            return Either.left(
              `Failed to serialize multimodal element: ${result.val}`,
            );
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

function serializeArgs(
  params: CachedParamInfo[],
  fnArgs: any[],
): WitValue.WitValue[] {
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

  return value.kind === 'tuple' && value.value.length > 0
    ? value.value[0]
    : value;
}

function deserializeRpcResult(
  rpcResult: Value.Value,
  typeInfoInternal: TypeInfoInternal,
): any {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      const dataValue = createSingleElementTupleDataValue({
        tag: 'component-model',
        val: Value.toWitValue(rpcResult),
      });

      return Either.getOrThrowWith(
        deserializeDataValue(dataValue, [
          {
            name: 'return-value',
            type: typeInfoInternal,
          },
        ]),
        (err) =>
          new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'unstructured-text':
      const textReference = convertValueToTextReference(rpcResult);

      const dataValueText = createSingleElementTupleDataValue({
        tag: 'unstructured-text',
        val: textReference,
      });

      return Either.getOrThrowWith(
        deserializeDataValue(dataValueText, [
          {
            name: 'return-value',
            type: typeInfoInternal,
          },
        ]),
        (err) =>
          new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'unstructured-binary':
      const binaryReference = convertValueToBinaryReference(rpcResult);

      const dataValueBinary = createSingleElementTupleDataValue({
        tag: 'unstructured-binary',
        val: binaryReference,
      });

      return Either.getOrThrowWith(
        deserializeDataValue(dataValueBinary, [
          {
            name: 'return-value',
            type: typeInfoInternal,
          },
        ]),
        (err) =>
          new Error(`Failed to deserialize return value of RPC call: ${err}`),
      )[0];

    case 'multimodal':
      const multimodalParamInfo: ParameterDetail[] = typeInfoInternal.types;

      switch (rpcResult.kind) {
        // A multimodal value is always a list
        case 'list':
          const values = rpcResult.value;

          const nameAndElementValues: [string, ElementValue][] = values.map(
            (value, idx) => {
              switch (value.kind) {
                case 'variant':
                  const caseIdx = value.caseIdx;
                  const paramDetail = multimodalParamInfo[caseIdx];
                  const caseValue = value.caseValue;

                  if (!caseValue) {
                    throw new Error(
                      `Missing case value in multimodal return value at index ${idx}`,
                    );
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
            },
          );

          const dataValue: DataValue = {
            tag: 'multimodal',
            val: nameAndElementValues,
          };

          return Either.getOrThrowWith(
            deserializeDataValue(dataValue, multimodalParamInfo),
            (err) =>
              new Error(
                `Failed to deserialize multimodal return value: ${err}`,
              ),
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
              throw new Error(
                `Invalid URL value type in value: ${urlValue.kind}`,
              );
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
              const languageCode = record.length > 1 ? record[1] : undefined;

              switch (data.kind) {
                case 'string':
                  const textData = data.value;

                  if (!languageCode) {
                    return {
                      tag: 'inline',
                      val: {
                        data: textData,
                      },
                    };
                  }

                  switch (languageCode.kind) {
                    case 'string':
                      const languageCodeStr = languageCode.value;
                      return {
                        tag: 'inline',
                        val: {
                          data: textData,
                          textType: { languageCode: languageCodeStr },
                        },
                      };

                    default:
                      throw new Error(
                        `Invalid inline text language code type: expected string`,
                      );
                  }

                default:
                  throw new Error(
                    `Invalid inline text data type: expected string`,
                  );
              }
            default:
              throw new Error(
                `Invalid inline text value type in value: ${inlineValue.kind}`,
              );
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
              throw new Error(
                `Invalid URL value type in value: ${urlValue.kind}`,
              );
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
                  throw new Error(
                    `Invalid inline binary mime type type: expected string`,
                  );
              }

            default:
              throw new Error(
                `Invalid inline binary value type in value: ${inlineValue.kind}`,
              );
          }
      }
  }

  throw new Error(`Unable to convert value to BinaryReference`);
}
