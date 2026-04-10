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

import {
  isConfig,
  isEmptyType,
  isOptionalWithQuestionMark,
  isPrincipal,
  TypeInfoInternal,
} from '../../typeInfoInternal';
import * as Either from '../../../newTypes/either';
import * as WitValue from '../../mapping/values/WitValue';
import {
  BinaryReference,
  DataValue,
  ElementValue,
  Principal,
  TextReference,
} from 'golem:agent/common@1.5.0';
import {
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
  matchesType,
} from './serializer';
import { UnstructuredText } from '../../../newTypes/textInput';
import { UnstructuredBinary } from '../../../newTypes/binaryInput';
import * as util from 'node:util';
import { getLanguageCodes, getMimeTypes } from '../../schema/helpers';
import { Config } from '../../../agentConfig';

export type ParameterDetail = {
  name: string;
  type: TypeInfoInternal;
};

/**
 *
 * Deserialize a DataValue to a list of typescript values
 *
 * A data-value may consist of multiple elements which can be converted to typescript-values
 *
 * @param dataValue A data value that corresponds to the set of method parameters and constructor parameters

 * @param paramTypes A data value is ever need to be deserialized only for method parameters or constructor parameters
 * (incoming values to dynamic invoke). Hence, it always expects a list of proper parameter names and its type info
 *
 * @param principal The principal of the caller - required to pass through whenever the required parameter is a Principal, as this will not exist in DataValue
 *
 * Implementation detail: The same functionality can be used to deserialize the result of the dynamic invoke - mainly
 * for testing purpose. In this case a fake parameter name can be provided when `dataValue.tag` is `tuple`.
 * And a proper list of `ParameterDetail` is required `dataValue.tag` is multi-modal - and it cannnot be fake.
 */
export function deserializeDataValue(
  dataValue: DataValue,
  paramTypes: ParameterDetail[],
  principal: Principal,
): any[] {
  switch (dataValue.tag) {
    case 'tuple':
      const inputElements = dataValue.val;
      const inputElementsLen = inputElements.length;

      // An index that's incremented corresponding to the schema
      // The index is incremented for each type unless it is autoinjected
      let schemaBasedIndex = 0;

      return paramTypes.map((parameterDetail) => {
        const parameterType = parameterDetail.type;

        if (schemaBasedIndex >= inputElementsLen) {
          if (isOptionalWithQuestionMark(parameterType)) {
            return undefined;
          }

          if (isEmptyType(parameterType)) {
            return undefined;
          }

          if (isPrincipal(parameterType)) {
            return principal;
          }

          if (isConfig(parameterType)) {
            return constructConfigType(parameterType);
          }

          throw new Error(
            `Internal error: Not enough elements in data value to deserialize parameter ${parameterDetail.name}`,
          );
        }

        const elementValue = inputElements[schemaBasedIndex];

        switch (parameterType.tag) {
          case 'multimodal':
            throw new Error(
              `Internal error: Unexpected multimodal type for parameter ${parameterDetail.name} in tuple data value`,
            );

          case 'principal':
            // If principal, we do not increment the data value element index,
            // because principal is not represented in data value
            return principal;

          case 'config':
            // If config, we do not increment the data value element index,
            // because config is not represented in data value
            return constructConfigType(parameterType);

          case 'unstructured-text':
            const unstructuredTextParamName = parameterDetail.name;

            if (elementValue.tag !== 'unstructured-text') {
              throw new Error(
                `Internal error: Expected unstructured-text element for parameter ${unstructuredTextParamName}, got ${util.format(elementValue)}`,
              );
            }

            const languageCodes = Either.getOrThrowWith(
              getLanguageCodes(parameterType.tsType),
              (err) =>
                new Error(
                  `Failed to get language codes for parameter ${unstructuredTextParamName}: ${err}`,
                ),
            );

            schemaBasedIndex += 1;

            return UnstructuredText.fromDataValue(
              unstructuredTextParamName,
              elementValue.val,
              languageCodes,
            );

          case 'unstructured-binary':
            const binaryParameterDetail = paramTypes[schemaBasedIndex];

            if (elementValue.tag !== 'unstructured-binary') {
              throw new Error(
                `Internal error: Expected unstructured-binary element for parameter ${binaryParameterDetail.name}, got ${util.format(elementValue)}`,
              );
            }

            const mimeTypes = Either.getOrThrowWith(
              getMimeTypes(binaryParameterDetail.type.tsType),
              (err) =>
                new Error(
                  `Failed to get mime types for parameter ${binaryParameterDetail.name}: ${err}`,
                ),
            );

            schemaBasedIndex += 1;

            return UnstructuredBinary.fromDataValue(
              binaryParameterDetail.name,
              elementValue.val,
              mimeTypes,
            );

          case 'analysed':
            if (elementValue.tag !== 'component-model') {
              throw new Error(
                `Internal error: Expected component-model element for parameter ${parameterDetail.name}, got ${util.format(elementValue)}`,
              );
            }

            schemaBasedIndex += 1;

            return WitValue.toTsValue(elementValue.val, parameterType.val);
        }
      });

    case 'multimodal':
      const multiModalElements = dataValue.val;

      const typeInfo = paramTypes[0].type;

      if (typeInfo.tag !== 'multimodal') {
        throw new Error(
          `Internal error: Expected multimodal type info for parameter ${paramTypes[0].name}, got ${util.format(typeInfo)}`,
        );
      }

      const multimodalParamTypes = typeInfo.types;

      // These are not separate parameters, but a single parameter of multimodal type
      const multiModalValue: any[] = multiModalElements.map(([name, elem]) => {
        switch (elem.tag) {
          case 'unstructured-text':
            const parameterDetail = multimodalParamTypes.find(
              (paramDetail) => paramDetail.name === name,
            );

            if (!parameterDetail) {
              throw new Error(
                `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => util.format(p)).join(', ')}`,
              );
            }

            const type = parameterDetail.type;
            const textRef = elem.val;

            const languageCodes = Either.getOrThrowWith(
              getLanguageCodes(type.tsType),
              (err) => new Error(`Failed to get language codes for parameter ${name}: ${err}`),
            );

            return {
              tag: name,
              val: UnstructuredText.fromDataValue(name, textRef, languageCodes),
            };

          case 'unstructured-binary':
            const binaryParameterDetail = multimodalParamTypes.find(
              (paramDetail) => paramDetail.name === name,
            );

            if (!binaryParameterDetail) {
              throw new Error(
                `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => util.format(p)).join(', ')}`,
              );
            }

            const binaryType = binaryParameterDetail.type;
            const binaryRef = elem.val;
            const mimeTypes = Either.getOrThrowWith(
              getMimeTypes(binaryType.tsType),
              (err) => new Error(`Failed to get mime types for parameter ${name}: ${err}`),
            );

            return {
              tag: name,
              val: UnstructuredBinary.fromDataValue(name, binaryRef, mimeTypes),
            };

          case 'component-model':
            const witValue = elem.val;

            const paramDetail = multimodalParamTypes.find(
              (paramDetail) => paramDetail.name === name,
            );

            if (!paramDetail) {
              throw new Error(
                `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => util.format(p)).join(', ')}`,
              );
            }

            const paramType = paramDetail.type;

            if (paramType.tag !== 'analysed') {
              throw new Error(
                `Internal error: Unknown parameter type for multimodal input ${util.format(elem.val)} with name ${name}`,
              );
            }

            let result = WitValue.toTsValue(witValue, paramType.val);

            return { tag: paramDetail.name, val: result };
        }
      });

      return [multiModalValue];
  }
}

function constructConfigType(typeInfoInternal: TypeInfoInternal & { tag: 'config' }): Config<any> {
  return new Config(typeInfoInternal.tsType.properties);
}

// Used to serialize the return type of a method back to DataValue
export function serializeToDataValue(tsValue: any, typeInfoInternal: TypeInfoInternal): DataValue {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      if (isEmptyType(typeInfoInternal)) {
        return {
          tag: 'tuple',
          val: [],
        };
      }

      const witValue = WitValue.fromTsValueDefault(tsValue, typeInfoInternal.val);
      const elementValue: ElementValue = {
        tag: 'component-model',
        val: witValue,
      };
      return {
        tag: 'tuple',
        val: [elementValue],
      };

    case 'principal':
      throw new Error(
        `Internal Error: Serialization of 'Principal' data should have never happened`,
      );

    case 'config':
      throw new Error(`Internal Error: Serialization of 'Config' data should have never happened`);

    case 'unstructured-text':
      return serializeTextReferenceToDataValue(tsValue);

    case 'unstructured-binary':
      return serializeBinaryReferenceToDataValue(tsValue);

    case 'multimodal':
      const multiModalTypeInfo = typeInfoInternal.types;

      const nameAndElementValues = serializeMultimodalToDataValue(tsValue, multiModalTypeInfo);

      return {
        tag: 'multimodal',
        val: nameAndElementValues,
      };
  }
}

function serializeBinaryReferenceToDataValue(tsValue: any): DataValue {
  const binaryReference: BinaryReference = serializeTsValueToBinaryReference(tsValue);

  const elementValue: ElementValue = {
    tag: 'unstructured-binary',
    val: binaryReference,
  };

  return {
    tag: 'tuple',
    val: [elementValue],
  };
}

function serializeTextReferenceToDataValue(value: any): DataValue {
  const textReference: TextReference = serializeTsValueToTextReference(value);

  const elementValue: ElementValue = {
    tag: 'unstructured-text',
    val: textReference,
  };

  return {
    tag: 'tuple',
    val: [elementValue],
  };
}

function serializeMultimodalToDataValue(
  value: any,
  paramDetails: ParameterDetail[],
): [string, ElementValue][] {
  const namesAndElements: [string, ElementValue][] = [];

  if (!Array.isArray(value)) {
    throw new Error(
      `Unable to serialize multimodal value ${util.format(value)}. Multimodal argument should be an array of values`,
    );
  }

  for (const elem of value) {
    let matchedParam: ParameterDetail | null = null;
    let matchedVal: any = undefined;

    for (const param of paramDetails) {
      const name = param.name;
      const type = param.type;

      const valOpt = getValFieldFromTaggedObject(elem, name);

      if (valOpt.tag === 'not-found') {
        continue;
      }

      const elemVal = valOpt.val;

      let isMatch = false;

      switch (type.tag) {
        case 'analysed':
          isMatch = matchesType(elemVal, type.val);
          break;

        case 'unstructured-binary': {
          const isObjectBinary = typeof elemVal === 'object' && elemVal !== null;
          isMatch =
            isObjectBinary &&
            'tag' in elemVal &&
            (elemVal.tag === 'url' || elemVal.tag === 'inline');
          break;
        }

        case 'unstructured-text': {
          const isObjectText = typeof elemVal === 'object' && elemVal !== null;
          isMatch =
            isObjectText && 'tag' in elemVal && (elemVal.tag === 'url' || elemVal.tag === 'inline');
          break;
        }

        case 'multimodal':
          throw new Error(`Nested multimodal types are not supported`);
      }

      if (isMatch) {
        matchedParam = param;
        matchedVal = elemVal;
        break;
      }
    }

    if (matchedParam === null) {
      throw new Error(
        `Unable to process multimodal input of elem ${util.format(elem)}. No matching type found in multimodal definition: ${paramDetails
          .map((t) => t.name)
          .join(', ')}`,
      );
    }

    const dataValue = serializeToDataValue(matchedVal, matchedParam.type);

    switch (dataValue.tag) {
      case 'tuple': {
        const element = dataValue.val[0];
        namesAndElements.push([matchedParam.name, element]);
        break;
      }
      case 'multimodal':
        throw new Error(`Nested multimodal types are not supported`);
      default:
        throw new Error(
          `Unexpected data value tag while serializing multimodal element: ${util.format(dataValue)}`,
        );
    }
  }

  return namesAndElements;
}

export function createSingleElementTupleDataValue(elementValue: ElementValue): DataValue {
  return {
    tag: 'tuple',
    val: [elementValue],
  };
}

/**
 * Gets the 'val' field from an object with a specific 'tag' field.
 *
 * @param value Example: { tag: 'someTag', val: someValue }
 * @param tagValue Example: 'someTag'
 */
function getValFieldFromTaggedObject(
  value: any,
  tagValue: string,
): { tag: 'found'; val: any } | { tag: 'not-found' } {
  if (typeof value === 'object' && value !== null) {
    if ('tag' in value && 'val' in value && value['tag'] === tagValue) {
      return { tag: 'found', val: value['val'] };
    }
  }

  return { tag: 'not-found' };
}
