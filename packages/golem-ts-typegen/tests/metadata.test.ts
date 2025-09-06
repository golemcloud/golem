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

import { describe, expect, it } from "vitest";
import {
  getBooleanType,
  getNumberType,
  getStringType,
  getTestListOfObjectType,
  getTestMapType,
  getObjectType,
  getTupleType,
  getUnionComplexType,
  getUnionType,
  getComplexObjectType,
  getInterfaceType,
  getClassType,
} from "./util.js";

import { Type } from "@golemcloud/golem-ts-types-core";

describe("golem-ts-typegen can work correctly read types from .metadata directory", () => {
  it("track interface type", () => {
    const stringType = getStringType();
    expect(stringType.kind).toEqual("string");
  });

  it("track number type", () => {
    const numberType = getNumberType();
    expect(numberType.kind).toEqual("number");
  });

  it("track boolean type", () => {
    const booleanType = getBooleanType();
    expect(booleanType.kind).toEqual("boolean");
  });

  it("track map type", () => {
    const mapType = getTestMapType();
    expect(mapType.kind).toEqual("map");
  });

  it("track tuple type", () => {
    const tupleType = getTupleType();
    expect(tupleType.kind).toEqual("tuple");
  });

  it("track array type", () => {
    const arrayType = getTestListOfObjectType();
    expect(arrayType.kind).toEqual("array");
  });

  it("track object type", () => {
    const objectType1 = getObjectType();
    expect(objectType1.kind).toEqual("object");

    const objectType2 = getComplexObjectType();
    expect(objectType2.kind).toEqual("object");
  });

  it("track union type", () => {
    const unionType1 = getUnionComplexType();
    expect(unionType1.kind).toEqual("union");

    const unionType2 = getUnionType();
    expect(unionType2.kind).toEqual("union");
  });

  it("track interface type", () => {
    const tupleType = getInterfaceType();
    expect(tupleType.kind).toEqual("interface");
  });

  it("track class type", () => {
    const classType = getClassType();
    expect(classType.kind).toEqual("class");
  });
});
