import { describe, it, expect } from "vitest";
import {
  convertToWaveFormat,
  convertToWaveFormatWithType,
  convertPayloadToWaveArgs,
  convertValuesToWaveArgs,
} from "../wave";
import { Parameter } from "@/types/component";

describe("WAVE format conversion", () => {
  describe("Primitive types", () => {
    it("converts strings correctly", () => {
      expect(convertToWaveFormat("hello")).toBe('"hello"');
      expect(convertToWaveFormat('test with "quotes"')).toBe(
        '"test with \\"quotes\\""',
      );
    });

    it("converts numbers correctly", () => {
      expect(convertToWaveFormat(42)).toBe("42");
      expect(convertToWaveFormat(3.14)).toBe("3.14");
    });

    it("converts booleans correctly", () => {
      expect(convertToWaveFormat(true)).toBe("true");
      expect(convertToWaveFormat(false)).toBe("false");
    });

    it("converts null/undefined correctly", () => {
      expect(convertToWaveFormat(null)).toBe("null");
      expect(convertToWaveFormat(undefined)).toBe("null");
    });
  });

  describe("Enum types", () => {
    it("converts enum values as unquoted identifiers", () => {
      const enumType = { type: "enum", cases: ["low", "medium", "high"] };
      expect(convertToWaveFormat("low", enumType)).toBe("low");
      expect(convertToWaveFormat("medium", enumType)).toBe("medium");
      expect(convertToWaveFormat("high", enumType)).toBe("high");
    });

    it("converts enum with parameter type information", () => {
      const parameter: Parameter = {
        name: "priority",
        type: "enum",
        typ: {
          type: "enum",
          cases: ["low", "medium", "high"],
        },
      };
      expect(convertToWaveFormatWithType("high", parameter)).toBe("high");
    });
  });

  describe("Option types", () => {
    it("converts none values correctly", () => {
      const optionType = { type: "option", inner: { type: "str" } };
      expect(convertToWaveFormat(null, optionType)).toBe("none");
      expect(convertToWaveFormat(undefined, optionType)).toBe("none");
    });

    it("converts some values correctly with type information", () => {
      const parameter: Parameter = {
        name: "optional_string",
        type: "option",
        typ: {
          type: "option",
          inner: { type: "str" },
        },
      };
      expect(convertToWaveFormatWithType("hello", parameter)).toBe('"hello"');
      expect(convertToWaveFormatWithType(null, parameter)).toBe("none");
    });
  });

  describe("List types", () => {
    it("converts arrays to list format", () => {
      expect(convertToWaveFormat(["one", "two", "three"])).toBe(
        '["one", "two", "three"]',
      );
      expect(convertToWaveFormat([1, 2, 3])).toBe("[1, 2, 3]");
    });

    it("converts lists with type information", () => {
      const parameter: Parameter = {
        name: "string_list",
        type: "list",
        typ: {
          type: "list",
          inner: { type: "str" },
        },
      };
      expect(convertToWaveFormatWithType(["hello", "world"], parameter)).toBe(
        '["hello", "world"]',
      );
    });
  });

  describe("Record types", () => {
    it("converts objects to record format", () => {
      const obj = { name: "John", age: 30 };
      expect(convertToWaveFormat(obj)).toBe('{name: "John", age: 30}');
    });

    it("converts records with field type information", () => {
      const parameter: Parameter = {
        name: "person",
        type: "record",
        typ: {
          type: "record",
          fields: [
            { name: "name", typ: { type: "str" } },
            { name: "priority", typ: { type: "enum", cases: ["low", "high"] } },
          ],
        },
      };
      const value = { name: "John", priority: "high" };
      expect(convertToWaveFormatWithType(value, parameter)).toBe(
        '{name: "John", priority: high}',
      );
    });
  });

  describe("Variant types", () => {
    it("converts variant objects correctly", () => {
      const variantType = {
        type: "variant",
        cases: [
          { name: "none", typ: { type: "unit" } },
          { name: "restricted", typ: { type: "list", inner: { type: "str" } } },
        ],
      };

      // No-payload variant
      expect(convertToWaveFormat({ none: null }, variantType)).toBe("none");

      // Variant with payload
      expect(
        convertToWaveFormat({ restricted: ["one", "two"] }, variantType),
      ).toBe('restricted(["one", "two"])');
    });
  });

  describe("Result types", () => {
    it("converts result objects correctly", () => {
      const resultType = {
        type: "result",
        ok: { type: "str" },
        err: { type: "str" },
      };

      expect(convertToWaveFormat({ ok: "success" }, resultType)).toBe(
        '"success"',
      );
      expect(convertToWaveFormat({ err: "failure" }, resultType)).toBe(
        'err("failure")',
      );
      expect(convertToWaveFormat({ ok: null }, resultType)).toBe("ok(null)");
    });
  });

  describe("Flags types", () => {
    it("converts flag arrays correctly", () => {
      const flagsType = {
        type: "flags",
        names: ["get", "post", "put", "delete"],
      };
      expect(convertToWaveFormat(["get", "post"], flagsType)).toBe(
        "{get, post}",
      );
      expect(convertToWaveFormat([], flagsType)).toBe("{}");
    });
  });

  describe("Tuple types", () => {
    it("converts arrays to tuple format", () => {
      const tupleType = {
        type: "tuple",
        fields: [
          { name: "_0", typ: { type: "u32" } },
          { name: "_1", typ: { type: "str" } },
          { name: "_2", typ: { type: "chr" } },
        ],
      };
      expect(convertToWaveFormat([1234, "hello", 103], tupleType)).toBe(
        '(1234, "hello", 103)',
      );
    });
  });

  describe("Payload conversion", () => {
    it("converts payload with type information", () => {
      const payload = {
        params: [
          { value: "hello", typ: { type: "str" } },
          { value: "high", typ: { type: "enum", cases: ["low", "high"] } },
          { value: [1, 2, 3], typ: { type: "list", inner: { type: "u32" } } },
        ],
      };

      const result = convertPayloadToWaveArgs(payload);
      expect(result).toEqual(['"hello"', "high", "[1, 2, 3]"]);
    });

    it("converts simple values array", () => {
      const values = ["hello", 42, true];
      const result = convertValuesToWaveArgs(values);
      expect(result).toEqual(['"hello"', "42", "true"]);
    });
  });

  describe("Complex nested types", () => {
    it("handles nested option within record", () => {
      const parameter: Parameter = {
        name: "user",
        type: "record",
        typ: {
          type: "record",
          fields: [
            { name: "name", typ: { type: "str" } },
            { name: "email", typ: { type: "option", inner: { type: "str" } } },
          ],
        },
      };

      const valueWithEmail = { name: "John", email: "john@example.com" };
      const valueWithoutEmail = { name: "John", email: null };

      expect(convertToWaveFormatWithType(valueWithEmail, parameter)).toBe(
        '{name: "John", email: "john@example.com"}',
      );
      expect(convertToWaveFormatWithType(valueWithoutEmail, parameter)).toBe(
        '{name: "John", email: none}',
      );
    });

    it("handles list of records", () => {
      const parameter: Parameter = {
        name: "users",
        type: "list",
        typ: {
          type: "list",
          inner: {
            type: "record",
            fields: [
              { name: "name", typ: { type: "str" } },
              { name: "active", typ: { type: "bool" } },
            ],
          },
        },
      };

      const value = [
        { name: "Alice", active: true },
        { name: "Bob", active: false },
      ];

      expect(convertToWaveFormatWithType(value, parameter)).toBe(
        '[{name: "Alice", active: true}, {name: "Bob", active: false}]',
      );
    });

    it("handles deeply nested enums in options", () => {
      const parameter: Parameter = {
        name: "config",
        type: "record",
        typ: {
          type: "record",
          fields: [
            {
              name: "priority",
              typ: {
                type: "option",
                inner: { type: "enum", cases: ["low", "medium", "high"] },
              },
            },
          ],
        },
      };

      const valueWithPriority = { priority: "high" };
      const valueWithoutPriority = { priority: null };

      expect(convertToWaveFormatWithType(valueWithPriority, parameter)).toBe(
        "{priority: high}",
      );
      expect(convertToWaveFormatWithType(valueWithoutPriority, parameter)).toBe(
        "{priority: none}",
      );
    });
  });

  describe("Complex nested recursive structures", () => {
    it("handles deeply nested options with variants", () => {
      const parameter: Parameter = {
        name: "complex",
        type: "option",
        typ: {
          type: "option",
          inner: {
            type: "variant",
            cases: [
              {
                name: "simple",
                typ: { type: "str" },
              },
              {
                name: "complex",
                typ: {
                  type: "record",
                  fields: [
                    {
                      name: "value",
                      typ: { type: "enum", cases: ["low", "medium", "high"] },
                    },
                    {
                      name: "optional",
                      typ: {
                        type: "option",
                        inner: { type: "list", inner: { type: "str" } },
                      },
                    },
                  ],
                },
              },
            ],
          },
        },
      };

      // Option containing variant with complex record
      const complexValue = {
        complex: {
          value: "high",
          optional: ["item1", "item2"],
        },
      };

      expect(convertToWaveFormatWithType(complexValue, parameter)).toBe(
        'some(complex({value: high, optional: some(["item1", "item2"])}))',
      );

      // Option containing variant with simple case
      const simpleValue = { simple: "test" };
      expect(convertToWaveFormatWithType(simpleValue, parameter)).toBe(
        'some(simple("test"))',
      );

      // None option
      expect(convertToWaveFormatWithType(null, parameter)).toBe("none");
    });

    it("handles nested records with multiple levels of options and enums", () => {
      const parameter: Parameter = {
        name: "userProfile",
        type: "record",
        typ: {
          type: "record",
          fields: [
            {
              name: "name",
              typ: { type: "str" },
            },
            {
              name: "preferences",
              typ: {
                type: "option",
                inner: {
                  type: "record",
                  fields: [
                    {
                      name: "theme",
                      typ: { type: "enum", cases: ["light", "dark", "auto"] },
                    },
                    {
                      name: "notifications",
                      typ: {
                        type: "option",
                        inner: {
                          type: "record",
                          fields: [
                            {
                              name: "email",
                              typ: { type: "bool" },
                            },
                            {
                              name: "priority",
                              typ: {
                                type: "enum",
                                cases: ["low", "normal", "high"],
                              },
                            },
                          ],
                        },
                      },
                    },
                  ],
                },
              },
            },
          ],
        },
      };

      const userWithFullPreferences = {
        name: "Alice",
        preferences: {
          theme: "dark",
          notifications: {
            email: true,
            priority: "high",
          },
        },
      };

      expect(
        convertToWaveFormatWithType(userWithFullPreferences, parameter),
      ).toBe(
        '{name: "Alice", preferences: some({theme: dark, notifications: some({email: true, priority: high})})}',
      );

      const userWithPartialPreferences = {
        name: "Bob",
        preferences: {
          theme: "light",
          notifications: null,
        },
      };

      expect(
        convertToWaveFormatWithType(userWithPartialPreferences, parameter),
      ).toBe(
        '{name: "Bob", preferences: some({theme: light, notifications: none})}',
      );

      const userWithoutPreferences = {
        name: "Charlie",
        preferences: null,
      };

      expect(
        convertToWaveFormatWithType(userWithoutPreferences, parameter),
      ).toBe('{name: "Charlie", preferences: none}');
    });

    it("handles variants with nested options and lists", () => {
      const parameter: Parameter = {
        name: "action",
        type: "variant",
        typ: {
          type: "variant",
          cases: [
            {
              name: "create",
              typ: {
                type: "record",
                fields: [
                  {
                    name: "data",
                    typ: { type: "str" },
                  },
                  {
                    name: "tags",
                    typ: {
                      type: "option",
                      inner: {
                        type: "list",
                        inner: {
                          type: "enum",
                          cases: ["urgent", "normal", "low"],
                        },
                      },
                    },
                  },
                ],
              },
            },
            {
              name: "update",
              typ: {
                type: "option",
                inner: { type: "str" },
              },
            },
            {
              name: "delete",
              typ: { type: "unit" },
            },
          ],
        },
      };

      // Variant with complex nested structure
      const createAction = {
        create: {
          data: "new item",
          tags: ["urgent", "normal"],
        },
      };

      expect(convertToWaveFormatWithType(createAction, parameter)).toBe(
        'create({data: "new item", tags: some([urgent, normal])})',
      );

      // Variant with optional value
      const updateAction = { update: "updated text" };
      expect(convertToWaveFormatWithType(updateAction, parameter)).toBe(
        'update("updated text")',
      );

      // Unit variant
      expect(convertToWaveFormatWithType("delete", parameter)).toBe("delete");
    });

    it("handles lists of complex nested structures", () => {
      const parameter: Parameter = {
        name: "items",
        type: "list",
        typ: {
          type: "list",
          inner: {
            type: "record",
            fields: [
              {
                name: "id",
                typ: { type: "str" },
              },
              {
                name: "status",
                typ: {
                  type: "variant",
                  cases: [
                    {
                      name: "pending",
                      typ: {
                        type: "option",
                        inner: { type: "str" },
                      },
                    },
                    {
                      name: "completed",
                      typ: { type: "bool" },
                    },
                    {
                      name: "cancelled",
                      typ: { type: "unit" },
                    },
                  ],
                },
              },
            ],
          },
        },
      };

      const listValue = [
        {
          id: "item1",
          status: { pending: "waiting for approval" },
        },
        {
          id: "item2",
          status: { completed: true },
        },
        {
          id: "item3",
          status: "cancelled", // This will be treated as unit variant
        },
      ];

      expect(convertToWaveFormatWithType(listValue, parameter)).toBe(
        '[{id: "item1", status: pending("waiting for approval")}, {id: "item2", status: completed(true)}, {id: "item3", status: cancelled}]',
      );
    });

    it("handles flags with complex structures", () => {
      const parameter: Parameter = {
        name: "permissions",
        type: "record",
        typ: {
          type: "record",
          fields: [
            {
              name: "flags",
              typ: {
                type: "flags",
                names: ["read", "write", "execute", "admin"],
              },
            },
            {
              name: "scope",
              typ: {
                type: "option",
                inner: {
                  type: "variant",
                  cases: [
                    {
                      name: "global",
                      typ: { type: "bool" },
                    },
                    {
                      name: "restricted",
                      typ: {
                        type: "list",
                        inner: { type: "str" },
                      },
                    },
                  ],
                },
              },
            },
          ],
        },
      };

      const permissionsValue = {
        flags: ["read", "write"],
        scope: {
          restricted: ["file1.txt", "file2.txt"],
        },
      };

      expect(convertToWaveFormatWithType(permissionsValue, parameter)).toBe(
        '{flags: {read, write}, scope: some(restricted(["file1.txt", "file2.txt"]))}',
      );
    });

    it("handles results with nested complex types", () => {
      const parameter: Parameter = {
        name: "operation",
        type: "result",
        typ: {
          type: "result",
          ok: {
            type: "record",
            fields: [
              {
                name: "data",
                typ: {
                  type: "option",
                  inner: {
                    type: "list",
                    inner: {
                      type: "enum",
                      cases: ["success", "warning", "info"],
                    },
                  },
                },
              },
            ],
          },
          err: {
            type: "variant",
            cases: [
              {
                name: "network",
                typ: { type: "str" },
              },
              {
                name: "validation",
                typ: {
                  type: "list",
                  inner: { type: "str" },
                },
              },
              {
                name: "timeout",
                typ: { type: "unit" },
              },
            ],
          },
        },
      };

      // Ok result with nested data
      const okResult = {
        ok: {
          data: ["success", "warning"],
        },
      };

      expect(convertToWaveFormatWithType(okResult, parameter)).toBe(
        "ok({data: some([success, warning])})",
      );

      // Error result with complex variant
      const errResult = {
        err: {
          validation: ["Field required", "Invalid format"],
        },
      };

      expect(convertToWaveFormatWithType(errResult, parameter)).toBe(
        'err(validation(["Field required", "Invalid format"]))',
      );
    });
  });
});
