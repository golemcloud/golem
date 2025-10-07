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

import { TypeInfoInternal } from '../../registry/typeInfoInternal';

import * as Either from '../../../newTypes/either';
import * as WitValue from '../../mapping/values/WitValue';
import {
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import {
  castTsValueToBinaryReference,
  castTsValueToTextReference,
} from './serializer';
import { getLanguageCodes, getMimeTypes } from '../../schema';
import { UnstructuredText } from '../../../newTypes/textInput';
import { UnstructuredBinary } from '../../../newTypes/binaryInput';
import * as util from 'node:util';

import * as Value from '../values/Value';

export type ParameterDetail = {
  parameterName: string;
  parameterTypeInfo: TypeInfoInternal;
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
 * Implementation detail: The same functionality can be used to deserialize the result of the dynamic invoke - mainly
 * for testing purpose. In this case a fake parameter name can be provided when `dataValue.tag` is `tuple`.
 * And a proper list of `ParameterDetail` is required `dataValue.tag` is multi-modal - and it cannnot be fake.
 */
export function deserializeDataValue(
  dataValue: DataValue,
  paramTypes: ParameterDetail[],
): Either.Either<any[], string> {
  switch (dataValue.tag) {
    case 'tuple':
      const elements = dataValue.val;

      return Either.all(
        elements.map((elem, idx) => {
          switch (elem.tag) {
            case 'unstructured-text':
              const parameterDetail = paramTypes[idx];

              const unstructuredTextParamName = parameterDetail.parameterName;

              const textRef = elem.val;

              const languageCodes: Either.Either<string[], string> =
                getLanguageCodes(parameterDetail.parameterTypeInfo.tsType);

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${unstructuredTextParamName}: ${languageCodes.val}`,
                );
              }

              return UnstructuredText.fromDataValue(
                unstructuredTextParamName,
                textRef,
                languageCodes.val,
              );

            case 'unstructured-binary':
              const binaryParameterDetail = paramTypes[idx];

              const binaryRef = elem.val;

              const mimeTypes: Either.Either<string[], string> = getMimeTypes(
                binaryParameterDetail.parameterTypeInfo.tsType,
              );

              if (Either.isLeft(mimeTypes)) {
                throw new Error(
                  `Failed to get mime types for parameter ${binaryParameterDetail.parameterName}: ${mimeTypes.val}`,
                );
              }

              return UnstructuredBinary.fromDataValue(
                binaryParameterDetail.parameterName,
                binaryRef,
                mimeTypes.val,
              );

            case 'component-model':
              const componentModelParameterDetail = paramTypes[idx];
              const type = componentModelParameterDetail.parameterTypeInfo;

              if (type.tag !== 'analysed') {
                throw new Error(
                  `Internal error: Unknown parameter type for ${componentModelParameterDetail.parameterName}`,
                );
              }

              const witValue = elem.val;
              return Either.right(WitValue.toTsValue(witValue, type.val));
          }
        }),
      );

    case 'multimodal':
      const multiModalElements = dataValue.val;

      return Either.all(
        multiModalElements.map(([name, elem]) => {
          switch (elem.tag) {
            case 'unstructured-text':
              const parameterDetail = paramTypes.find(
                (paramDetail) => paramDetail.parameterName === name,
              );

              if (!parameterDetail) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const type = parameterDetail.parameterTypeInfo;

              const textRef = elem.val;

              const languageCodes: Either.Either<string[], string> =
                getLanguageCodes(type.tsType);

              if (Either.isLeft(languageCodes)) {
                throw new Error(
                  `Failed to get language codes for parameter ${name}: ${languageCodes.val}`,
                );
              }

              return UnstructuredText.fromDataValue(
                name,
                textRef,
                languageCodes.val,
              );

            case 'unstructured-binary':
              const binaryParameterDetail = paramTypes.find(
                (paramDetail) => paramDetail.parameterName === name,
              );

              if (!binaryParameterDetail) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(elem.val)}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const binaryType = binaryParameterDetail.parameterTypeInfo;

              const binaryRef = elem.val;

              const mimeTypes = getMimeTypes(binaryType.tsType);

              if (Either.isLeft(mimeTypes)) {
                throw new Error(
                  `Failed to get mime types for parameter ${name}: ${mimeTypes.val}`,
                );
              }

              return UnstructuredBinary.fromDataValue(
                name,
                binaryRef,
                mimeTypes.val,
              );

            case 'component-model':
              const witValue = elem.val;

              const paramDetail = paramTypes.find(
                (paramDetail) => paramDetail.parameterName === name,
              );

              if (!paramDetail) {
                throw new Error(
                  `Unable to process multimodal input of elem ${util.format(Value.fromWitValue(elem.val))}. Unknown parameter \`${name}\` in multimodal input. Available: ${paramTypes.map((p) => JSON.stringify(p)).join(', ')}`,
                );
              }

              const paramType = paramDetail.parameterTypeInfo;

              if (paramType.tag !== 'analysed') {
                throw new Error(
                  `Internal error: Unknown parameter type for multimodal input ${util.format(Value.fromWitValue(elem.val))} with name ${name}`,
                );
              }

              return Either.right(WitValue.toTsValue(witValue, paramType.val));
          }
        }),
      );
  }
}

export function serializeToDataValue(
  tsValue: any,
  typeInfoInternal: TypeInfoInternal,
): Either.Either<DataValue, string> {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      return Either.map(
        WitValue.fromTsValueDefault(tsValue, typeInfoInternal.val),
        (witValue) => {
          let elementValue: ElementValue = {
            tag: 'component-model',
            val: witValue,
          };

          return {
            tag: 'tuple',
            val: [elementValue],
          };
        },
      );
    case 'unstructured-text':
      return Either.right(serializeTextReferenceToDataValue(tsValue));
    case 'unstructured-binary':
      return Either.right(serializeBinaryReferenceToDataValue(tsValue));

    // TODO;
    case 'multimodal':
      return Either.left(
        'Serialization of multimodal type is not supported here',
      );
  }
}

function serializeBinaryReferenceToDataValue(tsValue: any): DataValue {
  const binaryReference: BinaryReference =
    castTsValueToBinaryReference(tsValue);

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
  const textReference: TextReference = castTsValueToTextReference(value);

  const elementValue: ElementValue = {
    tag: 'unstructured-text',
    val: textReference,
  };

  return {
    tag: 'tuple',
    val: [elementValue],
  };
}
