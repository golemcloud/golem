import { AnalysedType } from './analysedType';

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';

import * as Either from "../../../newTypes/either";
import { TypeMappingScope } from './scope';

// Refer to `typeMapperImpl` for the only implementation.
export type TypeMapper = (t: CoreType.Type, scope: TypeMappingScope | undefined) => Either.Either<AnalysedType, string>;
