import { TypeInfoInternal } from '../../registry/typeInfoInternal';

import * as Either from '../../../newTypes/either';
import * as WitValue from '../../mapping/values/WitValue';
import {
  BinaryReference,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import {
  castTsValueToBinaryReference,
  castTsValueToTextReference,
} from './unstructured';

export function convertTsValueToElementValue(
  tsValue: any,
  typeInfoInternal: TypeInfoInternal,
): Either.Either<ElementValue, string> {
  switch (typeInfoInternal.tag) {
    case 'analysed':
      return Either.map(
        WitValue.fromTsValue(tsValue, typeInfoInternal.val),
        (witValue) => {
          let elementValue: ElementValue = {
            tag: 'component-model',
            val: witValue,
          };

          return elementValue;
        },
      );
    case 'unstructured-text':
      return Either.right(convertTextReferenceToElementValue(tsValue));
    case 'unstructured-binary':
      return Either.right(convertBinaryReferenceToElementValue(tsValue));
  }
}

export function convertBinaryReferenceToElementValue(
  tsValue: any,
): ElementValue {
  const binaryReference: BinaryReference =
    castTsValueToBinaryReference(tsValue);

  return {
    tag: 'unstructured-binary',
    val: binaryReference,
  };
}

export function convertTextReferenceToElementValue(value: any): ElementValue {
  const textReference: TextReference = castTsValueToTextReference(value);

  return {
    tag: 'unstructured-text',
    val: textReference,
  };
}
