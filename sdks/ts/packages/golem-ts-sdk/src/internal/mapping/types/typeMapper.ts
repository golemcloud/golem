import { AnalysedType } from './analysedType';

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';

import * as Either from '../../../newTypes/either';
import { TypeScope } from './scope';

// Refer to `typeMapperImpl` for the only implementation.
export type TypeMapper = (
  t: CoreType.Type,
  scope: TypeScope | undefined,
) => Either.Either<AnalysedType, string>;
