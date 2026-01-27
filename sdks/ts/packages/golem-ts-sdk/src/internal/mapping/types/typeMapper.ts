import { AnalysedType } from './analysedType';

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';

import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import { TypeMappingScope } from './scope';

export type TypeMapper = (t: CoreType.Type, scope: Option.Option<TypeMappingScope>) => Either.Either<AnalysedType, string>;
