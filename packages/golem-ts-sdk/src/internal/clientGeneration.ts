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
import { Datetime, WasmRpc, AgentId } from 'golem:rpc/types@0.2.2';
import * as Either from '../newTypes/either';
import * as WitValue from './mapping/values/WitValue';
import * as Option from '../newTypes/option';
import {
  getAgentType,
  makeAgentId,
  RegisteredAgentType,
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
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
  matchesType,
  serializeBinaryReferenceTsValue,
  serializeDefaultTsValue,
  serializeTextReferenceTsValue,
} from './mapping/values/serializer';
import { TypeInfoInternal } from './registry/typeInfoInternal';
import { match } from 'node:assert';
import {
  deserializeDataValue,
  ParameterDetail,
} from './mapping/values/dataValue';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  ctor: T,
) {
  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const agentClassName = new AgentClassName(ctor.name);

    const metadataOpt = Option.fromNullable(TypeMetadata.get(ctor.name));

    if (Option.isNone(metadataOpt)) {
      throw new Error(
        `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
      );
    }

    const metadata = metadataOpt.val;

    const workerIdEither = getAgentId(agentClassName, args, metadata);

    if (Either.isLeft(workerIdEither)) {
      throw new Error(workerIdEither.val);
    }

    const workerId = workerIdEither.val;

    return new Proxy(instance, {
      get(target, prop) {
        const val = target[prop];

        if (typeof val === 'function') {
          return getMethodProxy(metadata, prop, agentClassName, workerId);
        }
        return undefined;
      },
    });
  };
}

function getMethodProxy(
  classMetadata: ClassMetadata,
  prop: string | symbol,
  agentClassName: AgentClassName,
  agentId: AgentId,
): RemoteMethod<any[], any> {
  const methodSignature = classMetadata.methods.get(prop.toString());

  const methodParams = methodSignature?.methodParams;

  if (!methodParams) {
    throw new Error(
      `Unresolved method ${String(
        prop,
      )} in RPC call. Make sure this method exists and is not private/protected`,
    );
  }

  const paramInfo = Array.from(methodParams);

  const methodName = prop.toString();

  const methodNameKebab = convertAgentMethodNameToKebab(methodName);

  const functionName = `${agentClassName.asWit}.{${methodNameKebab}}`;

  const returnTypeInfoInternal = AgentMethodRegistry.getReturnType(
    agentClassName,
    methodName,
  );

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
          for (const elem of arg) {
            const index = types.findIndex((type) => {
              const [, internal] = type;
              switch (internal.tag) {
                case 'analysed':
                  return matchesType(elem, internal.val);

                case 'unstructured-binary':
                  const isObjectBinary =
                    typeof elem === 'object' && elem !== null;
                  const keysBinary = Object.keys(elem);
                  return (
                    isObjectBinary &&
                    keysBinary.includes('tag') &&
                    (elem['tag'] === 'url' || elem['tag'] === 'inline')
                  );

                case 'unstructured-text':
                  const isObject = typeof elem === 'object' && elem !== null;
                  const keys = Object.keys(elem);
                  return (
                    isObject &&
                    keys.includes('tag') &&
                    (elem['tag'] === 'url' || elem['tag'] === 'inline')
                  );

                case 'multimodal':
                  throw new Error(`Nested multimodal types are not supported`);
              }
            });

            const result = convertToValue(arg[index], types[index][1]);

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
          return Either.left(
            `Multimodal argument should be an array of values`,
          );
        }

        return Either.right({
          kind: 'list',
          value: values,
        });
    }
  }

  function serializeArgs(fnArgs: any[]): WitValue.WitValue[] {
    const parameterWitValuesEither = Either.all(
      fnArgs.map((fnArg, index) => {
        const param = paramInfo[index];
        const typeInfo = AgentMethodParamRegistry.getParamType(
          agentClassName,
          methodName,
          param[0],
        );

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for parameter ${param[0]} in method ${String(
              prop,
            )}`,
          );
        }

        return Either.map(convertToValue(fnArg, typeInfo), (v) =>
          Value.toWitValue(v),
        );
      }),
    );

    if (Either.isLeft(parameterWitValuesEither)) {
      throw new Error('Failed to encode args: ' + parameterWitValuesEither.val);
    }
    return parameterWitValuesEither.val;
  }

  async function invokeAndAwait(...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);

    const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(
      functionName,
      parameterWitValues,
    );

    const rpcResultPollable = rpcResultFuture.subscribe();
    await rpcResultPollable.promise();

    const rpcResult = rpcResultFuture.get();
    if (!rpcResult) {
      throw new Error(
        `Failed to invoke ${functionName} in agent ${agentId.agentId}`,
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

    if (!returnTypeInfoInternal) {
      throw new Error(
        `Return type of method ${String(prop)}  not supported in remote calls`,
      );
    }

    const rpcValueUnwrapped = unwrapResult(rpcWitValue);

    return deserializeRpcResult(rpcValueUnwrapped, returnTypeInfoInternal);
  }

  function invokeFireAndForget(...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);
    wasmRpc.invoke(functionName, parameterWitValues);
  }

  function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);
    wasmRpc.scheduleInvocation(ts, functionName, parameterWitValues);
  }

  const methodFn: any = (...args: any[]) => invokeAndAwait(...args);

  methodFn.trigger = (...args: any[]) => invokeFireAndForget(...args);
  methodFn.schedule = (ts: Datetime, ...args: any[]) =>
    invokeSchedule(ts, ...args);

  return methodFn as RemoteMethod<any[], any>;
}

// constructorArgs is an array of any, we can have more control depending on its types
// Probably this implementation is going to exist in various forms in Golem. Not sure if there
// would be a way to reuse - may be a host function that retrieves the worker-id
// given value in JSON format, and the wit-type of each value and agent-type name?
function getAgentId(
  agentClassName: AgentClassName,
  constructorArgs: any[],
  classMetadata: ClassMetadata,
): Either.Either<AgentId, string> {
  // PlaceHolder implementation that finds the container-id corresponding to the agentType!
  // We need a host function - given an agent-type, it should return a component-id as proved in the prototype.
  // But we don't have that functionality yet, hence just retrieving the current
  // component-id (for now)
  const optionalRegisteredAgentType = Option.fromNullable(
    getAgentType(agentClassName.value),
  );

  if (Option.isNone(optionalRegisteredAgentType)) {
    return Either.left(
      `There are no components implementing ${agentClassName.value}`,
    );
  }

  const registeredAgentType: RegisteredAgentType =
    optionalRegisteredAgentType.val;

  const constructorParamInfo = classMetadata.constructorArgs;

  const constructorParamTypes = constructorParamInfo.map((param) => {
    const typeInfoInternal = AgentConstructorParamRegistry.getParamType(
      agentClassName,
      param.name,
    );

    if (!typeInfoInternal) {
      throw new Error(
        `Unresolved type for constructor parameter ${param.name} in agent class ${agentClassName.value}`,
      );
    }
    return typeInfoInternal;
  });

  // It's a bit odd that the agent-id creation takes a DataValue,
  // while remote calls takes WitValue regardless of whether it is component-type
  // or unstructured-types.
  const constructorParamWitValuesResult: Either.Either<ElementValue[], string> =
    Either.all(
      constructorArgs.map((arg, index) => {
        const typeInfoInternal = constructorParamTypes[index];

        switch (typeInfoInternal.tag) {
          case 'analysed':
            return Either.map(
              WitValue.fromTsValueDefault(arg, typeInfoInternal.val),
              (witValue) => {
                let elementValue: ElementValue = {
                  tag: 'component-model',
                  val: witValue,
                };

                return elementValue;
              },
            );
          case 'unstructured-text':
            const textReference: TextReference =
              serializeTsValueToTextReference(arg);

            const elementValue: Either.Either<ElementValue, string> =
              Either.right({
                tag: 'unstructured-text',
                val: textReference,
              });

            return elementValue;

          case 'unstructured-binary':
            const binaryReference: BinaryReference =
              serializeTsValueToBinaryReference(arg);

            const elementValueBinary: Either.Either<ElementValue, string> =
              Either.right({
                tag: 'unstructured-binary',
                val: binaryReference,
              });

            return elementValueBinary;

          case 'multimodal':
            return Either.left(
              'Multimodal constructor parameters are not supported in remote calls',
            );
        }
      }),
    );

  if (Either.isLeft(constructorParamWitValuesResult)) {
    throw new Error(
      'Failed to create remote agent: ' + constructorParamWitValuesResult.val,
    );
  }

  const constructorDataValue: DataValue = {
    tag: 'tuple',
    val: constructorParamWitValuesResult.val,
  };

  const agentId = makeAgentId(agentClassName.value, constructorDataValue);

  return Either.right({
    componentId: registeredAgentType.implementedBy,
    agentId: agentId,
  });
}

function convertAgentMethodNameToKebab(methodName: string): string {
  return methodName
    .replace(/([a-z])([A-Z])/g, '$1-$2')
    .replace(/[\s_]+/g, '-')
    .toLowerCase();
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
      const dataValue: DataValue = {
        tag: 'tuple',
        val: [
          {
            tag: 'component-model',
            val: Value.toWitValue(rpcResult),
          },
        ],
      };

      const result = Either.map(
        deserializeDataValue(dataValue, [
          {
            parameterName: 'return-value',
            parameterTypeInfo: typeInfoInternal,
          },
        ]),
        (values) => values[0],
      );

      if (Either.isLeft(result)) {
        throw new Error(
          `Failed to deserialize return value from rpc call: ${result.val}`,
        );
      }

      return result.val;

    case 'unstructured-text':
      const textReferenceEither = convertValueToTextReference(rpcResult);

      if (Either.isLeft(textReferenceEither)) {
        throw new Error(
          `Failed to convert return value to TextReference: ${textReferenceEither.val}`,
        );
      }

      const dataValueText: DataValue = {
        tag: 'tuple',
        val: [
          {
            tag: 'unstructured-text',
            val: textReferenceEither.val,
          },
        ],
      };

      const textResult = Either.map(
        deserializeDataValue(dataValueText, [
          {
            parameterName: 'return-value',
            parameterTypeInfo: typeInfoInternal,
          },
        ]),
        (values) => values[0],
      );

      if (Either.isLeft(textResult)) {
        throw new Error(
          `Failed to deserialize return value: ${textResult.val}`,
        );
      }

      return textResult.val;

    case 'unstructured-binary':
      const binaryReferenceEither = convertValueToBinaryReference(rpcResult);

      if (Either.isLeft(binaryReferenceEither)) {
        throw new Error(
          `Failed to convert return value to BinaryReference: ${binaryReferenceEither.val}`,
        );
      }

      const dataValueBinary: DataValue = {
        tag: 'tuple',
        val: [
          {
            tag: 'unstructured-binary',
            val: binaryReferenceEither.val,
          },
        ],
      };

      const paramInfo = [
        {
          parameterName: 'return-value',
          parameterTypeInfo: typeInfoInternal,
        },
      ];

      const binaryResult = Either.map(
        deserializeDataValue(dataValueBinary, paramInfo),
        (values) => values[0],
      );

      if (Either.isLeft(binaryResult)) {
        throw new Error(
          `Failed to deserialize return value: ${binaryResult.val}`,
        );
      }

      return binaryResult.val;

    case 'multimodal':
      const multimodalParamInfo: ParameterDetail[] = typeInfoInternal.types.map(
        ([name, typeInfo]) => ({
          parameterName: name,
          parameterTypeInfo: typeInfo,
        }),
      );

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
                    paramDetail.parameterTypeInfo,
                  );

                  return [paramDetail.parameterName, elementValue];

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

          const multimodalTsValue = Either.map(
            deserializeDataValue(dataValue, multimodalParamInfo),
            (values) => values[0],
          );

          if (Either.isLeft(multimodalTsValue)) {
            throw new Error(
              `Failed to deserialize return value: ${multimodalTsValue.val}`,
            );
          }

          return multimodalTsValue.val;
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
      const textReferenceEither =
        convertValueToTextReference(rpcValueUnwrapped);

      if (Either.isLeft(textReferenceEither)) {
        throw new Error(
          `Failed to convert return value to TextReference: ${textReferenceEither.val}`,
        );
      }

      return {
        tag: 'unstructured-text',
        val: textReferenceEither.val,
      };

    case 'unstructured-binary':
      const binaryReferenceEither =
        convertValueToBinaryReference(rpcValueUnwrapped);

      if (Either.isLeft(binaryReferenceEither)) {
        throw new Error(
          `Failed to convert return value to BinaryReference: ${binaryReferenceEither.val}`,
        );
      }

      return {
        tag: 'unstructured-binary',
        val: binaryReferenceEither.val,
      };

    case 'multimodal':
      // DataValue::Multimodal cannot encode recursive multimodals
      throw new Error(`Nested multimodal values are not supported`);
  }
}

function convertValueToTextReference(
  value: Value.Value,
): Either.Either<TextReference, string> {
  switch (value.kind) {
    case 'variant':
      const idx = value.caseIdx;
      switch (idx) {
        case 0:
          // url
          const urlValue = value.caseValue;

          if (!urlValue) {
            return Either.left(`Unable to extract URL from value`);
          }

          switch (urlValue.kind) {
            case 'string':
              return Either.right({
                tag: 'url',
                val: urlValue.value,
              });

            default:
              return Either.left(
                `Invalid URL value type in value: ${urlValue.kind}`,
              );
          }

        case 1:
          // inline
          const inlineValue = value.caseValue;

          if (!inlineValue) {
            return Either.left(`Unable to extract inline text from value`);
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
                    return Either.right({
                      tag: 'inline',
                      val: {
                        data: textData,
                      },
                    });
                  }

                  switch (languageCode.kind) {
                    case 'string':
                      const languageCodeStr = languageCode.value;
                      return Either.right({
                        tag: 'inline',
                        val: {
                          data: textData,
                          textType: { languageCode: languageCodeStr },
                        },
                      });

                    default:
                      return Either.left(
                        `Invalid inline text language code type: expected string`,
                      );
                  }

                default:
                  return Either.left(
                    `Invalid inline text data type: expected string`,
                  );
              }
            default:
              return Either.left(
                `Invalid inline text value type in value: ${inlineValue.kind}`,
              );
          }
      }
  }

  return Either.left(`Unable to convert value to TextReference`);
}

function convertValueToBinaryReference(
  value: Value.Value,
): Either.Either<BinaryReference, string> {
  switch (value.kind) {
    case 'variant':
      const idx = value.caseIdx;
      switch (idx) {
        case 0:
          // url
          const urlValue = value.caseValue;

          if (!urlValue) {
            return Either.left(`Unable to extract URL from value`);
          }

          switch (urlValue.kind) {
            case 'string':
              return Either.right({
                tag: 'url',
                val: urlValue.value,
              });

            default:
              return Either.left(
                `Invalid URL value type in value: ${urlValue.kind}`,
              );
          }

        case 1:
          // inline
          const inlineValue = value.caseValue;

          if (!inlineValue) {
            return Either.left(`Unable to extract inline binary from value`);
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
                return Either.left(`Unable to extract mime type from value`);
              }

              switch (mimeType.kind) {
                case 'string':
                  return Either.right({
                    tag: 'inline',
                    val: {
                      data: uint8Array,
                      binaryType: {
                        mimeType: mimeType.value,
                      },
                    },
                  });
                default:
                  return Either.left(
                    `Invalid inline binary mime type type: expected string`,
                  );
              }

            default:
              return Either.left(
                `Invalid inline binary value type in value: ${inlineValue.kind}`,
              );
          }
      }
  }

  return Either.left(`Unable to convert value to BinaryReference`);
}
