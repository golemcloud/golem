// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { UnstructuredBinary as SdkUnstructuredBinary } from '../newTypes/binaryInput';
import type { UnstructuredText as SdkUnstructuredText } from '../newTypes/textInput';

export type * from '../internal/schema-model/model';
export {
  t,
  v,
  field,
  variantCase,
  schemaType,
  emptyMetadata,
} from '../internal/schema-model/model';
export {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
} from '../internal/schema-model/wit';
export { Uuid } from '../uuid';
export { UnstructuredText } from '../newTypes/textInput';
export { UnstructuredBinary } from '../newTypes/binaryInput';
export type UnstructuredTextType<LC extends string[] = []> = SdkUnstructuredText<LC>;
export type UnstructuredBinaryType<MT extends string[] | string = string> =
  SdkUnstructuredBinary<MT>;
export * from './schema';
export * from './agent';
export * from './tool';
