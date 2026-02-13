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

import { isEmptyType, isOptionalWithQuestionMark, isPrincipal, TypeInfoInternal } from '../../typeInfoInternal';

import * as Either from '../../../newTypes/either';
import * as WitValue from '../../mapping/values/WitValue';
import {
  BinaryReference,
  DataValue,
  ElementValue,
  Principal,
  TextReference,
} from 'golem:agent/common';
import {
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
  matchesType,
} from './serializer';
import { UnstructuredText } from '../../../newTypes/textInput';
import { UnstructuredBinary } from '../../../newTypes/binaryInput';
import * as util from 'node:util';

import * as Value from '../values/Value';
import { getLanguageCodes, getMimeTypes } from '../../schema/helpers';

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
): Either.Either<any[], string> {
  switch (dataValue.tag) {
    case 'tuple':
      const inputElements = dataValue.val;
      const inputElementsLen = inputElements.length;

      // An index that's incremented corresponding to the schema
      // The index is incremented for each type unless it is of type `Principal`
      let schemaBasedIndex = 0;

      return Either.all(
        paramTypes.map((parameterDetail) => {
          const parameterType = parameterDetail.type;

          if (schemaBasedIndex >= inputElementsLen) {
            if (isOptionalWithQuestionMark(parameterType)) {
              return Either.right(undefined);
            }

            if (isEmptyType(parameterType)) {
              return Either.right(undefined);
            }

            if (isPrincipal(parameterType)) {
              return Either.right(principal)
            }

            throw new Error(
              `Internal error: Not enough elements in data value to deserialize parameter ${parameterDetail.name}`,
            );
          }

          const elementValue = inputElements[schemaBasedIndex];

          switch (parameterType.tag) {
            case 'multimodal':
              return Either.left(
                `Internal error: Unexpected multimodal type for parameter ${parameterDetail.name} in tuple data value`,
              );

            case 'principal':
              // If principal, we do not increment the data value elemnt index,
              // because principal is not represented in data value
              return Either.right(principal);

            case 'unstructured-text':
              const unstructuredTextParamName = parameterDetail.name;

              if (elementValue.tag !== 'unstructured-text') {
                throw new Error(
                  `Internal error: Expected unstructured-text element for parameter ${unstructuredTextParamName}, got ${util.format(elementValue)}`,
                );
              }

              const languageCodes: Either.Either<string[], string> = getLanguageCodes(
                parameterType.tsType,
              );

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${unstructuredTextParamName}: ${languageCodes.val}`,
                );
              }

              schemaBasedIndex += 1;

              return UnstructuredText.fromDataValue(
                unstructuredTextParamName,
                elementValue.val,
                languageCodes.val,
              );

            case 'unstructured-binary':
              const binaryParameterDetail = paramTypes[schemaBasedIndex];

              if (elementValue.tag !== 'unstructured-binary') {
                throw new Error(
                  `Internal error: Expected unstructured-binary element for parameter ${binaryParameterDetail.name}, got ${util.format(elementValue)}`,
                );
              }

              const mimeTypes: Either.Either<string[], string> = getMimeTypes(
                binaryParameterDetail.type.tsType,
              );

              if (Either.isLeft(mimeTypes)) {
                throw new Error(
                  `Failed to get mime types for parameter ${binaryParameterDetail.name}: ${mimeTypes.val}`,
                );
              }

              schemaBasedIndex += 1;

              return UnstructuredBinary.fromDataValue(
                binaryParameterDetail.name,
                elementValue.val,
                mimeTypes.val,
              );

            case 'analysed':
              if (elementValue.tag !== 'component-model') {
                throw new Error(
                  `Internal error: Expected component-model element for parameter ${parameterDetail.name}, got ${util.format(elementValue)}`,
                );
              }

              schemaBasedIndex += 1;

              return Either.right(WitValue.toTsValue(elementValue.val, parameterType.val));
          }
        }),
      );

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
      const multiModalValue: Either.Either<any[], string> = Either.all(
        multiModalElements.map(([name, elem]) => {
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

              const languageCodes: Either.Either<string[], string> = getLanguageCodes(type.tsType);

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${name}: ${languageCodes.val}`,
                );
              }

              const unstructuredText = UnstructuredText.fromDataValue(
                name,
                textRef,
                languageCodes.val,
              );

              return Either.map(unstructuredText, (v) => ({
                tag: name,
                val: v,
              }));

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
              const mimeTypes = getMimeTypes(binaryType.tsType);

              if (Either.isLeft(mimeTypes)) {
                throw new Error(`Failed to get mime types for parameter ${name}: ${mimeTypes.val}`);
              }

              const unstructuredBinary = UnstructuredBinary.fromDataValue(
                name,
                binaryRef,
                mimeTypes.val,
              );

              return Either.map(unstructuredBinary, (v) => ({
                tag: name,
                val: v,
              }));

            case 'component-model':
              const witValue = elem.val;

              const paramDetail = multimodalParamTypes.find(
                (paramDetail) => paramDetail.name === name,
              );

              if (!paramDetail) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(Value.fromWitValue(elem.val))}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => util.format(p)).join(', ')}`,
                );
              }

              const paramType = paramDetail.type;

              if (paramType.tag !== 'analysed') {
                throw new Error(
                  `Internal error: Unknown parameter type for multimodal input ${util.format(Value.fromWitValue(elem.val))} with name ${name}`,
                );
              }

              let result = WitValue.toTsValue(witValue, paramType.val);

              let multimodal_result = { tag: paramDetail.name, val: result };

              return Either.right(multimodal_result);
          }
        }),
      );

      return Either.map(multiModalValue, (v) => [v]);
  }
}

// Used to serialize the return type of a method back to DataValue
export function serializeToDataValue(
  tsValue: any,
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataValue, string> {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      if (isEmptyType(typeInfoInternal)) {
        return Either.right({
          tag: 'tuple',
          val: [],
        });
      }

      return Either.map(WitValue.fromTsValueDefault(tsValue, typeInfoInternal.val), (witValue) => {
        let elementValue: ElementValue = {
          tag: 'component-model',
          val: witValue,
        };

        return {
          tag: 'tuple',
          val: [elementValue],
        };
      });

    case 'principal':
      return Either.left(
        `Internal Error: Serialization of 'Principal' data should have never happened`,
      );

    case 'unstructured-text':
      return Either.right(serializeTextReferenceToDataValue(tsValue));

    case 'unstructured-binary':
      return Either.right(serializeBinaryReferenceToDataValue(tsValue));

    case 'multimodal':
      const multiModalTypeInfo = typeInfoInternal.types;

      const nameAndElementValues = serializeMultimodalToDataValue(tsValue, multiModalTypeInfo);

      return Either.right({
        tag: 'multimodal',
        val: nameAndElementValues,
      });
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

    const result = serializeToDataValue(matchedVal, matchedParam.type);

    if (Either.isLeft(result)) {
      throw new Error(
        `Failed to serialize multimodal element: ${util.format(elem)}. Error: ${result.val}`,
      );
    }

    const dataValue = result.val;

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
