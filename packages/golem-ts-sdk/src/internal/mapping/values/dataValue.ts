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

export function convertTsValueToDataValue(
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
      return Either.right(convertTextReferenceToDataValue(tsValue));
    case 'unstructured-binary':
      return Either.right(convertBinaryReferenceToDataValue(tsValue));
  }
}

function convertBinaryReferenceToDataValue(tsValue: any): DataValue {
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

function convertTextReferenceToDataValue(value: any): DataValue {
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
