import { AnalysedType } from './analysedType';

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';

import * as Either from "../../../newTypes/either";
import { TypeMappingScope } from './scope';

export type TypeMapper = (t: CoreType.Type, scope: TypeMappingScope | undefined) => Either.Either<AnalysedType, string>;
