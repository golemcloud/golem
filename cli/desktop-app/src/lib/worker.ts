/* eslint-disable @typescript-eslint/no-explicit-any */
import {
  Case,
  ComponentExportFunction,
  Field,
  Typ,
  TypeField,
} from "@/types/component.ts";
// Raw type structure from external sources
interface RawType {
  type?: string;
  items?: RawType[];
  fields?: RawTypeField[];
  cases?: RawTypeCase[];
  inner?: RawType;
  names?: string[];
  ok?: RawType;
  err?: RawType;
}

interface RawTypeField {
  name: string;
  typ: RawType;
}

interface RawTypeCase {
  name: string;
  typ?: RawType;
}

interface RawParameterData {
  parameters: Array<{ typ: RawType }>;
}

export interface RawTypesInput {
  parameters: Array<{ typ: RawType }>;
}

function buildJsonSkeleton(field: Field): unknown {
  const { type, fields, cases, names, inner } = field.typ;
  const typeStr = type?.toLowerCase();
  switch (typeStr) {
    case "str":
    case "chr":
      return "";

    case "bool":
      return false;

    case "f64":
    case "f32":
    case "u64":
    case "s64":
    case "u32":
    case "s32":
    case "u16":
    case "s16":
    case "u8":
    case "s8":
      return 0;

    case "record": {
      const obj: Record<string, unknown> = {};
      fields?.forEach((subField: Field) => {
        obj[subField.name] = buildJsonSkeleton(subField);
      });
      return obj;
    }

    case "tuple": {
      if (!fields) return [];
      return fields.map((subField: Field) => buildJsonSkeleton(subField));
    }

    case "list": {
      if (inner) {
        return [buildJsonSkeleton({ ...field, typ: inner })];
      }
      return [];
    }

    case "option": {
      return null;
    }

    case "flags": {
      return names ? [names[0]] : [];
    }

    case "enum": {
      if (cases && cases.length > 0) {
        // For the example for priority enum, show a practical example
        if (
          (cases as string[]).includes("low") &&
          (cases as string[]).includes("medium") &&
          (cases as string[]).includes("high")
        ) {
          return "low";
        }
        return cases[0];
      }
      return "";
    }

    case "variant": {
      if (!cases || cases.length === 0) return null;
      const selectedCase = cases[0];
      if (typeof selectedCase !== "object" || !selectedCase.typ) return null;
      return { [selectedCase.name]: buildJsonSkeleton(selectedCase) };
    }

    case "result": {
      return {
        ok:
          field.typ && field.typ.ok
            ? buildJsonSkeleton({
                ...field.typ.ok,
                typ: field.typ.ok,
                name: "",
              })
            : null,
        err: cases
          ? `enum (${(cases as Case[]).map(c => c.name).join(", ")})`
          : "",
      };
    }

    default:
      return null;
  }
}

/**
 * Convert a component function’s parameter definition
 * into a default JSON array for user editing.
 */
export function parseToJsonEditor(data: ComponentExportFunction) {
  return data.parameters.map(param => buildJsonSkeleton(param));
}

export function safeFormatJSON(input: string): string {
  try {
    const parsed = JSON.parse(input);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return input; // Return as-is if parse fails
  }
}

export function getCaretCoordinates(
  element: HTMLTextAreaElement,
  position: number,
) {
  const div = document.createElement("div");
  const styles = getComputedStyle(element);
  const properties = [
    "direction",
    "boxSizing",
    "width",
    "height",
    "overflowX",
    "overflowY",
    "borderTopWidth",
    "borderRightWidth",
    "borderBottomWidth",
    "borderLeftWidth",
    "borderStyle",
    "paddingTop",
    "paddingRight",
    "paddingBottom",
    "paddingLeft",
    "fontStyle",
    "fontVariant",
    "fontWeight",
    "fontStretch",
    "fontSize",
    "fontSizeAdjust",
    "lineHeight",
    "fontFamily",
    "textAlign",
    "textTransform",
    "textIndent",
    "textDecoration",
    "letterSpacing",
    "wordSpacing",
    "tabSize",
    "MozTabSize",
  ];

  div.id = "input-textarea-caret-position-mirror-div";
  document.body.appendChild(div);

  const style = div.style;
  style.whiteSpace = "pre-wrap";
  style.wordWrap = "break-word";
  style.position = "absolute";
  style.visibility = "hidden";

  properties.forEach((prop: string) => {
    if (Object.prototype.hasOwnProperty.call(styles, prop)) {
      const value = styles.getPropertyValue(prop);
      if (value) {
        style.setProperty(prop, value);
      }
    }
  });

  div.textContent = element.value.substring(0, position);
  const span = document.createElement("span");
  span.textContent = element.value.substring(position) || ".";
  div.appendChild(span);

  const coordinates = {
    top: span.offsetTop + Number.parseInt(styles["borderTopWidth"]),
    left: span.offsetLeft + Number.parseInt(styles["borderLeftWidth"]),
    height: Number.parseInt(styles["lineHeight"]),
  };

  document.body.removeChild(div);

  return coordinates;
}

function parseType(typ: RawType): Typ | null {
  if (!typ) return null;

  const typeMap: Record<string, string> = {
    Bool: "bool",
    S8: "i8",
    S16: "i16",
    S32: "i32",
    S64: "i64",
    U8: "u8",
    U16: "u16",
    U32: "u32",
    U64: "u64",
    F32: "f32",
    F64: "f64",
    Char: "char",
    Str: "string",
  };

  if (typ.type && typeMap[typ.type]) {
    const mappedType = typeMap[typ.type];
    if (mappedType) return { type: mappedType };
  }

  switch (typ.type) {
    case "Tuple":
      if (!typ.items) return { type: "Tuple" };
      return {
        type: "Tuple",
        fields: typ.items.map((item, index) => ({
          name: `${index}`,
          typ: parseType(item) || { type: "unknown" },
        })),
      };

    case "List":
      if (!typ.inner) return { type: "List" };
      return {
        type: "List",
        inner: parseType(typ.inner) || { type: "unknown" },
      };

    case "Flags":
      return {
        type: "Flags",
        names: typ.names || [],
      };

    case "Option":
      if (!typ.inner) return { type: "Option" };
      return {
        type: "Option",
        inner: parseType(typ.inner) || { type: "unknown" },
      };

    case "Result":
      return {
        type: "Result",
        ok: typ.ok
          ? parseType(typ.ok) || { type: "unknown" }
          : { type: "unknown" },
        err: typ.err
          ? parseType(typ.err) || { type: "unknown" }
          : { type: "unknown" },
      };

    case "Record":
      if (!typ.fields) return { type: "Record", fields: [] };
      return {
        type: "Record",
        fields: typ.fields.map((field: RawTypeField) => ({
          name:
            field.typ.inner?.type === "List"
              ? `${field.name}<${field.typ.type}<List>>`
              : field.typ.type === "Option" ||
                  field.typ.type === "List" ||
                  field.typ.type === "Flags" ||
                  field.typ.type === "Enum"
                ? `${field.name}<${field.typ.type}>`
                : field.name,
          typ: parseType(field.typ) || { type: "unknown" },
        })),
      };

    case "Enum":
      if (!typ.cases) return { type: "Enum", cases: [] };
      return {
        type: "Enum",
        cases: typ.cases.map(c => (typeof c === "string" ? c : c.name)),
      };

    case "Variant":
      if (!typ.cases) return { type: "Variant", cases: [] };
      return {
        type: "Variant",
        cases: typ.cases.map((variant: RawTypeCase) => ({
          name: variant.typ?.type
            ? `${variant.name}<${variant.typ.type}>`
            : variant.name,
          typ: variant.typ
            ? parseType(variant.typ) || { type: "unknown" }
            : { type: "unknown" },
        })),
      };

    default:
      return { type: "unknown" };
  }
}

export function parseTooltipTypesData(data: RawParameterData) {
  return data.parameters.map((item: { typ: RawType }) => parseType(item.typ));
}

export function parseTypesData(input: RawTypesInput): { items: TypeField[] } {
  function transformType(typ: RawType): TypeField {
    if (!typ || typeof typ !== "object")
      return { name: "", typ: { type: "unknown" } };

    switch (typ.type) {
      case "Str":
      case "Bool":
      case "S8":
      case "S16":
      case "S32":
      case "S64":
      case "U8":
      case "U16":
      case "U32":
      case "U64":
      case "F32":
      case "F64":
      case "Char":
        return { name: "", typ: { type: typ.type } };

      case "List":
        return {
          name: "",
          typ: {
            type: "List",
            inner: typ.inner
              ? transformType(typ.inner).typ
              : { type: "unknown" },
          } as any,
        };

      case "Option":
        return {
          name: "",
          typ: {
            type: "Option",
            inner: typ.inner
              ? transformType(typ.inner).typ
              : { type: "unknown" },
          } as any,
        };

      case "Enum":
        return { name: "", typ: { type: "Enum", cases: typ.cases } as any };

      case "Flags":
        return { name: "", typ: { type: "Flags", names: typ.names } as any };

      case "Record":
        return {
          name: "",
          typ: {
            type: "Record",
            fields: (typ.fields || []).map((field: RawTypeField) => ({
              name: field.name,
              typ: transformType(field.typ).typ,
            })),
          } as any,
        };

      case "Variant":
        return {
          name: "",
          typ: {
            type: "Variant",
            cases: (typ.cases || []).map((variant: RawTypeCase) => ({
              name: variant.name,
              typ: variant.typ
                ? transformType(variant.typ).typ
                : { type: "unknown" },
            })),
          } as any,
        };

      case "Result":
        return {
          name: "",
          typ: {
            type: "Result",
            ok: typ.ok ? transformType(typ.ok).typ : { type: "unknown" },
            err: typ.err ? transformType(typ.err).typ : { type: "unknown" },
          } as any,
        };

      case "Tuple":
        return {
          name: "",
          typ: {
            type: "Tuple",
            fields: (typ.items || []).map((item: RawType, index: number) => ({
              name: `${index}`,
              typ: transformType(item).typ,
            })),
          } as any,
        };

      default:
        return { name: "", typ: { type: "Unknown" } };
    }
  }

  return {
    items: input.parameters.map((param: { typ: RawType }) =>
      transformType(param.typ),
    ),
  };
}

function normalizeType(type: string): string {
  return type.toLowerCase();
}

export function validateJsonStructure(
  data: unknown,
  field: TypeField,
): string | null {
  const { type, fields, cases, names, inner } = field.typ;
  const normalizedType = normalizeType(type);

  const isInteger = (num: number) => Number.isInteger(num);
  const isUnsigned = (num: number) => num >= 0 && isInteger(num);
  const fitsBitSize = (num: number, bits: number, signed: boolean) => {
    const min = signed ? -(2 ** (bits - 1)) : 0;
    const max = signed ? 2 ** (bits - 1) - 1 : 2 ** bits - 1;
    return num >= min && num <= max;
  };

  switch (normalizedType) {
    case "str":
    case "chr":
      if (typeof data !== "string") {
        return `Expected a string for field "${field.name}", but got ${typeof data}`;
      }
      break;

    case "bool":
      if (typeof data !== "boolean") {
        return `Expected a boolean for field "${field.name}", but got ${typeof data}`;
      }
      break;

    case "f64":
    case "f32":
      if (typeof data !== "number") {
        return `Expected a number for field "${field.name}", but got ${typeof data}`;
      }
      break;

    case "u64":
    case "u32":
    case "u16":
    case "u8": {
      const bitSize = parseInt(normalizedType.slice(1), 10);
      if (
        typeof data !== "number" ||
        !isUnsigned(data) ||
        !fitsBitSize(data, bitSize, false)
      ) {
        return `Expected an unsigned ${bitSize}-bit integer for field "${field.name}", but got ${data}`;
      }
      break;
    }

    case "s64":
    case "s32":
    case "s16":
    case "s8": {
      const bitSize = parseInt(normalizedType.slice(1), 10);
      if (
        typeof data !== "number" ||
        !isInteger(data) ||
        !fitsBitSize(data, bitSize, true)
      ) {
        return `Expected a signed ${bitSize}-bit integer for field "${field.name}", but got ${data}`;
      }
      break;
    }

    case "record": {
      if (typeof data !== "object" || data === null || Array.isArray(data)) {
        return `Expected an object for field "${field.name}", but got ${typeof data}`;
      }
      if (!fields) break;
      const dataObj = data as Record<string, unknown>;
      for (const subField of fields) {
        const error = validateJsonStructure(dataObj[subField.name], subField);
        if (error) return error;
      }
      break;
    }

    case "tuple": {
      if (!Array.isArray(data)) {
        return `Expected an array for field "${field.name}", but got ${typeof data}`;
      }
      if (!fields) break;
      if (data.length !== fields.length) {
        return `Expected ${fields.length} elements in tuple for field "${field.name}", but got ${data.length}`;
      }
      for (let i = 0; i < fields.length; i++) {
        const fieldItem = fields[i];
        if (fieldItem) {
          const error = validateJsonStructure(data[i], fieldItem);
          if (error) return error;
        }
      }
      break;
    }

    case "list": {
      if (!Array.isArray(data)) {
        return `Expected an array for field "${field.name}", but got ${typeof data}`;
      }
      if (inner) {
        for (const item of data) {
          const error = validateJsonStructure(item, { ...field, typ: inner });
          if (error) return error;
        }
      }
      break;
    }

    case "option": {
      if (data !== null && data !== undefined) {
        const error = validateJsonStructure(data, {
          ...field,
          typ: field.typ.inner!,
        });
        if (error) return error;
      }
      break;
    }

    case "flags": {
      if (!Array.isArray(data)) {
        return `Expected an array for field "${field.name}", but got ${typeof data}`;
      }
      if (names && !data.every(item => names.includes(item))) {
        return `Expected flags to be one of [${names.join(", ")}] for field "${field.name}"`;
      }
      break;
    }

    case "enum": {
      if (typeof data !== "string") {
        return `Expected a string for field "${field.name}", but got ${typeof data}`;
      }
      if (cases) {
        const validValues = cases.map(c =>
          typeof c === "string" ? c : c.name,
        );
        if (!validValues.includes(data)) {
          return `Expected enum value to be one of [${validValues.join(", ")}] for field "${field.name}"`;
        }
      }
      break;
    }

    case "variant": {
      if (!cases || cases.length === 0) break;
      if (typeof data !== "object" || data === null || Array.isArray(data)) {
        return `Expected an object for field "${field.name}", but got ${typeof data}`;
      }
      const dataObj = data as Record<string, unknown>;
      const caseNames = cases.map(c => (typeof c === "string" ? c : c.name));
      const keys = Object.keys(dataObj);
      if (keys.length === 0) {
        return `Expected variant to have one of [${caseNames.join(", ")}] for field "${field.name}"`;
      }
      const selectedCase = keys[0];
      if (!selectedCase || !caseNames.includes(selectedCase)) {
        return `Expected variant to be one of [${caseNames.join(", ")}] for field "${field.name}"`;
      }
      const selectedCaseField = cases.find(
        (c): c is { name: string; typ: Typ } =>
          typeof c !== "string" && c.name === selectedCase,
      );
      if (selectedCaseField) {
        const error = validateJsonStructure(
          dataObj[selectedCase],
          selectedCaseField,
        );
        if (error) return error;
      }
      break;
    }

    case "result": {
      if (typeof data !== "object" || data === null || Array.isArray(data)) {
        return `Expected an object for field "${field.name}", but got ${typeof data}`;
      }
      const dataObj = data as Record<string, unknown>;
      if ("ok" in dataObj && dataObj.ok !== null && dataObj.ok !== undefined) {
        const error = validateJsonStructure(dataObj.ok, {
          ...field,
          typ: field.typ.ok!,
          name: "",
        });
        if (error) return error;
      }
      if (
        "err" in dataObj &&
        dataObj.err !== null &&
        dataObj.err !== undefined
      ) {
        if (typeof dataObj.err !== "string") {
          return `Expected a string for field "${field.name}.err", but got ${typeof dataObj.err}`;
        }
      }
      break;
    }

    default:
      return `Unknown type "${normalizedType}" for field "${field.name}"`;
  }

  return null; // No error
}
