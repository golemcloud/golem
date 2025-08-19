import { describe, it, expect } from "vitest";
import {
  parseToJsonEditor,
  safeFormatJSON,
  getCaretCoordinates,
  parseTooltipTypesData,
  parseTypesData,
  validateJsonStructure,
} from "../worker";
import { ComponentExportFunction, TypeField } from "@/types/component";

describe("worker utilities", () => {
  describe("parseToJsonEditor", () => {
    it("should handle string parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          { name: "param1", typ: { type: "Str" }, type: "String" },
          { name: "param2", typ: { type: "Chr" }, type: "String" },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual(["", ""]);
    });

    it("should handle boolean parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          { name: "param1", typ: { type: "Bool" }, type: "Boolean" },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([false]);
    });

    it("should handle numeric parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          { name: "param1", typ: { type: "F64" }, type: "Number" },
          { name: "param2", typ: { type: "U32" }, type: "Number" },
          { name: "param3", typ: { type: "S16" }, type: "Number" },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([0, 0, 0]);
    });

    it("should handle record parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Record",
              fields: [
                { name: "field1", typ: { type: "Str" } },
                { name: "field2", typ: { type: "U32" } },
              ],
            },
            type: "Record",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([{ field1: "", field2: 0 }]);
    });

    it("should handle tuple parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Tuple",
              fields: [
                { name: "item1", typ: { type: "Str" } },
                { name: "item2", typ: { type: "Bool" } },
              ],
            },
            type: "Tuple",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([["", false]]);
    });

    it("should handle list parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "List",
              inner: { type: "Str" },
            },
            type: "List",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([[""]]);
    });

    it("should handle option parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Option",
              inner: { type: "Str" },
            },
            type: "Option",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([null]);
    });

    it("should handle flags parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Flags",
              names: ["flag1", "flag2", "flag3"],
            },
            type: "Flags",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([["flag1"]]);
    });

    it("should handle enum parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Enum",
              cases: ["case1", "case2", "case3"],
            },
            type: "Enum",
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual(["case1"]);
    });

    it("should handle variant parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            type: "Variant",
            typ: {
              type: "Variant",
              cases: [
                { name: "variant1", typ: { type: "Str" } },
                { name: "variant2", typ: { type: "U32" } },
              ],
            },
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([{ variant1: "" }]);
    });

    it("should handle result parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [
          {
            name: "param1",
            type: "Result",
            typ: {
              type: "Result",
              ok: { type: "Str" },
              cases: [
                {
                  name: "error1",
                  typ: {
                    type: "Str",
                  },
                },
                { name: "error2", typ: { type: "Str" } },
              ],
            },
          },
        ],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([{ ok: "", err: "enum (error1, error2)" }]);
    });

    it("should handle empty parameters", () => {
      const data: ComponentExportFunction = {
        name: "test-function",
        parameters: [],
        results: [],
      };

      const result = parseToJsonEditor(data);
      expect(result).toEqual([]);
    });
  });

  describe("safeFormatJSON", () => {
    it("should format valid JSON", () => {
      const input = '{"name":"test","value":123}';
      const expected = '{\n  "name": "test",\n  "value": 123\n}';

      const result = safeFormatJSON(input);
      expect(result).toBe(expected);
    });

    it("should return original string for invalid JSON", () => {
      const input = '{"name":"test",invalid}';

      const result = safeFormatJSON(input);
      expect(result).toBe(input);
    });

    it("should handle empty string", () => {
      const input = "";

      const result = safeFormatJSON(input);
      expect(result).toBe(input);
    });
  });

  describe("getCaretCoordinates", () => {
    it("should calculate caret coordinates for textarea", () => {
      // Create a mock textarea element
      const textarea = document.createElement("textarea");
      textarea.value = "Hello\nWorld\nTest";
      textarea.style.font = "14px monospace";
      textarea.style.padding = "10px";
      textarea.style.border = "1px solid black";

      // Mock getComputedStyle
      const mockComputedStyle = {
        direction: "ltr",
        boxSizing: "border-box",
        width: "200px",
        height: "100px",
        overflowX: "hidden",
        overflowY: "auto",
        borderTopWidth: "1px",
        borderRightWidth: "1px",
        borderBottomWidth: "1px",
        borderLeftWidth: "1px",
        borderStyle: "solid",
        paddingTop: "10px",
        paddingRight: "10px",
        paddingBottom: "10px",
        paddingLeft: "10px",
        fontStyle: "normal",
        fontVariant: "normal",
        fontWeight: "normal",
        fontStretch: "normal",
        fontSize: "14px",
        fontSizeAdjust: "none",
        lineHeight: "18px",
        fontFamily: "monospace",
        textAlign: "left",
        textTransform: "none",
        textIndent: "0px",
        textDecoration: "none",
        letterSpacing: "normal",
        wordSpacing: "normal",
        tabSize: "4",
        MozTabSize: "4",
      };

      // Mock getComputedStyle function
      const originalGetComputedStyle = window.getComputedStyle;
      window.getComputedStyle = vi.fn(
        () =>
          ({
            ...mockComputedStyle,
            getPropertyValue: vi.fn(
              (prop: string) =>
                mockComputedStyle[prop as keyof typeof mockComputedStyle] || "",
            ),
          }) as unknown as CSSStyleDeclaration,
      );

      document.body.appendChild(textarea);

      const result = getCaretCoordinates(textarea, 6); // Position after "Hello\n"

      expect(result).toHaveProperty("top");
      expect(result).toHaveProperty("left");
      expect(result).toHaveProperty("height");
      expect(typeof result.top).toBe("number");
      expect(typeof result.left).toBe("number");
      expect(typeof result.height).toBe("number");

      // Clean up
      document.body.removeChild(textarea);
      window.getComputedStyle = originalGetComputedStyle;
    });
  });

  describe("parseTooltipTypesData", () => {
    it("should parse basic types", () => {
      const data = {
        parameters: [
          { name: "param1", typ: { type: "Bool" } },
          { name: "param2", typ: { type: "Str" } },
          { name: "param3", typ: { type: "U32" } },
        ],
      };

      const result = parseTooltipTypesData(data);
      expect(result).toEqual([
        { type: "bool" },
        { type: "string" },
        { type: "u32" },
      ]);
    });

    it("should parse complex types", () => {
      const data = {
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Record",
              fields: [
                { name: "field1", typ: { type: "Str" } },
                { name: "field2", typ: { type: "U32" } },
              ],
            },
          },
          {
            name: "param2",
            typ: {
              type: "List",
              inner: { type: "Str" },
            },
          },
        ],
      };

      const result = parseTooltipTypesData(data);
      expect(result).toEqual([
        {
          type: "Record",
          fields: [
            { name: "field1", typ: { type: "string" } },
            { name: "field2", typ: { type: "u32" } },
          ],
        },
        {
          type: "List",
          inner: { type: "string" },
        },
      ]);
    });
  });

  describe("parseTypesData", () => {
    it("should transform basic types", () => {
      const input = {
        parameters: [
          { name: "param1", typ: { type: "Bool" } },
          { name: "param2", typ: { type: "Str" } },
        ],
      };

      const result = parseTypesData(input);
      expect(result).toEqual({
        items: [
          { name: "", typ: { type: "Bool" } },
          { name: "", typ: { type: "Str" } },
        ],
      });
    });

    it("should transform complex types", () => {
      const input = {
        parameters: [
          {
            name: "param1",
            typ: {
              type: "Record",
              fields: [
                { name: "field1", typ: { type: "Str" } },
                { name: "field2", typ: { type: "U32" } },
              ],
            },
          },
        ],
      };

      const result = parseTypesData(input);
      expect(result).toEqual({
        items: [
          {
            name: "",
            typ: {
              type: "Record",
              fields: [
                { name: "field1", typ: { type: "Str" } },
                { name: "field2", typ: { type: "U32" } },
              ],
            },
          },
        ],
      });
    });
  });

  describe("validateJsonStructure", () => {
    it("should validate string fields", () => {
      const field: TypeField = { name: "testField", typ: { type: "Str" } };

      expect(validateJsonStructure("valid string", field)).toBeNull();
      expect(validateJsonStructure(123, field)).toBe(
        'Expected a string for field "testField", but got number',
      );
    });

    it("should validate boolean fields", () => {
      const field: TypeField = { name: "testField", typ: { type: "Bool" } };

      expect(validateJsonStructure(true, field)).toBeNull();
      expect(validateJsonStructure("true", field)).toBe(
        'Expected a boolean for field "testField", but got string',
      );
    });

    it("should validate numeric fields", () => {
      const field: TypeField = { name: "testField", typ: { type: "F64" } };

      expect(validateJsonStructure(123.45, field)).toBeNull();
      expect(validateJsonStructure("123", field)).toBe(
        'Expected a number for field "testField", but got string',
      );
    });

    it("should validate unsigned integer fields", () => {
      const field: TypeField = { name: "testField", typ: { type: "U8" } };

      expect(validateJsonStructure(255, field)).toBeNull();
      expect(validateJsonStructure(-1, field)).toBe(
        'Expected an unsigned 8-bit integer for field "testField", but got -1',
      );
      expect(validateJsonStructure(256, field)).toBe(
        'Expected an unsigned 8-bit integer for field "testField", but got 256',
      );
    });

    it("should validate signed integer fields", () => {
      const field: TypeField = { name: "testField", typ: { type: "S8" } };

      expect(validateJsonStructure(-128, field)).toBeNull();
      expect(validateJsonStructure(127, field)).toBeNull();
      expect(validateJsonStructure(-129, field)).toBe(
        'Expected a signed 8-bit integer for field "testField", but got -129',
      );
      expect(validateJsonStructure(128, field)).toBe(
        'Expected a signed 8-bit integer for field "testField", but got 128',
      );
    });

    it("should validate record fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Record",
          fields: [
            { name: "field1", typ: { type: "Str" } },
            { name: "field2", typ: { type: "U32" } },
          ],
        },
      };

      expect(
        validateJsonStructure({ field1: "test", field2: 123 }, field),
      ).toBeNull();
      expect(
        validateJsonStructure({ field1: "test", field2: "invalid" }, field),
      ).toBe(
        'Expected an unsigned 32-bit integer for field "field2", but got invalid',
      );
      expect(validateJsonStructure("not an object", field)).toBe(
        'Expected an object for field "testField", but got string',
      );
    });

    it("should validate tuple fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Tuple",
          fields: [
            { name: "item1", typ: { type: "Str" } },
            { name: "item2", typ: { type: "Bool" } },
          ],
        },
      };

      expect(validateJsonStructure(["test", true], field)).toBeNull();
      expect(validateJsonStructure(["test"], field)).toBe(
        'Expected 2 elements in tuple for field "testField", but got 1',
      );
      expect(validateJsonStructure(["test", "invalid"], field)).toBe(
        'Expected a boolean for field "item2", but got string',
      );
    });

    it("should validate list fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "List",
          inner: { type: "Str" },
        },
      };

      expect(validateJsonStructure(["item1", "item2"], field)).toBeNull();
      expect(validateJsonStructure(["item1", 123], field)).toBe(
        'Expected a string for field "testField", but got number',
      );
      expect(validateJsonStructure("not an array", field)).toBe(
        'Expected an array for field "testField", but got string',
      );
    });

    it("should validate option fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Option",
          inner: { type: "Str" },
        },
      };

      expect(validateJsonStructure(null, field)).toBeNull();
      expect(validateJsonStructure("valid", field)).toBeNull();
      expect(validateJsonStructure(123, field)).toBe(
        'Expected a string for field "testField", but got number',
      );
    });

    it("should validate flags fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Flags",
          names: ["flag1", "flag2", "flag3"],
        },
      };

      expect(validateJsonStructure(["flag1", "flag2"], field)).toBeNull();
      expect(validateJsonStructure(["flag1", "invalid"], field)).toBe(
        'Expected flags to be one of [flag1, flag2, flag3] for field "testField"',
      );
      expect(validateJsonStructure("not an array", field)).toBe(
        'Expected an array for field "testField", but got string',
      );
    });

    it("should validate enum fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Enum",
          cases: ["case1", "case2", "case3"],
        },
      };

      expect(validateJsonStructure("case1", field)).toBeNull();
      expect(validateJsonStructure("invalid", field)).toBe(
        'Expected enum value to be one of [case1, case2, case3] for field "testField"',
      );
    });

    it("should validate variant fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Variant",
          cases: [
            { name: "variant1", typ: { type: "Str" } },
            { name: "variant2", typ: { type: "U32" } },
          ],
        },
      };

      expect(validateJsonStructure({ variant1: "test" }, field)).toBeNull();
      expect(validateJsonStructure({ variant2: 123 }, field)).toBeNull();
      expect(validateJsonStructure({ invalid: "test" }, field)).toBe(
        'Expected variant to be one of [variant1, variant2] for field "testField"',
      );
      expect(validateJsonStructure({ variant1: 123 }, field)).toBe(
        'Expected a string for field "variant1", but got number',
      );
    });

    it("should validate result fields", () => {
      const field: TypeField = {
        name: "testField",
        typ: {
          type: "Result",
          ok: { type: "Str" },
          err: { type: "Str" },
        },
      };

      expect(
        validateJsonStructure({ ok: "success", err: null }, field),
      ).toBeNull();
      expect(
        validateJsonStructure({ ok: null, err: "error" }, field),
      ).toBeNull();
      expect(validateJsonStructure({ ok: 123, err: null }, field)).toBe(
        'Expected a string for field "", but got number',
      );
    });

    it("should return error for unknown types", () => {
      const field: TypeField = {
        name: "testField",
        typ: { type: "UnknownType" },
      };

      expect(validateJsonStructure("test", field)).toBe(
        'Unknown type "unknowntype" for field "testField"',
      );
    });
  });
});
