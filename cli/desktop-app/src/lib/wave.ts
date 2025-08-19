/**
 * WAVE (WebAssembly Value Encoding) format utilities
 * Clean recursive implementation for comprehensive WIT type support
 */

import { Parameter, Typ } from "@/types/component";

/**
 * Main conversion function with recursive type handling
 */
export function convertToWaveFormat(value: unknown, typ?: Typ): string {
  return convertValueWithType(value, typ);
}

/**
 * Core recursive conversion that handles all WIT types properly
 */
function convertValueWithType(value: unknown, typ?: Typ): string {
  // Handle null/undefined
  if (value === null || value === undefined) {
    return typ?.type === "option" ? "none" : "null";
  }

  // No type info - basic conversion
  if (!typ) {
    return convertBasicValue(value);
  }

  // Handle each WIT type with proper recursion
  switch (typ.type) {
    case "str":
    case "string":
      return `"${String(value).replace(/"/g, '\\"')}"`;

    case "bool":
      return String(value);

    case "u8":
    case "u16":
    case "u32":
    case "u64":
    case "s8":
    case "s16":
    case "s32":
    case "s64":
    case "f32":
    case "f64":
      return String(value);

    case "chr":
      return typeof value === "number"
        ? String(value)
        : String(value.toString().charCodeAt(0));

    case "enum":
      return String(value);

    case "option": {
      if (value === null || value === undefined) return "none";
      const innerValue = convertValueWithType(value, typ.inner);
      return typ.inner && isSimpleType(typ.inner.type)
        ? innerValue
        : `some(${innerValue})`;
    }

    case "list": {
      if (!Array.isArray(value)) return convertBasicValue(value);
      const items = value.map(item => convertValueWithType(item, typ.inner));
      return `[${items.join(", ")}]`;
    }

    case "record":
      return convertRecord(value, typ);

    case "variant":
      return convertVariant(value, typ);

    case "result":
      return convertResult(value, typ);

    case "tuple":
      return convertTuple(value, typ);

    case "flags":
      if (!Array.isArray(value)) return convertBasicValue(value);
      return `{${value.map(flag => String(flag)).join(", ")}}`;

    case "handle":
      return `"${String(value).replace(/"/g, '\\"')}"`;

    case "unit":
      return "null";

    default:
      console.warn("Unknown type:", typ.type);
      return convertBasicValue(value);
  }
}

function convertRecord(value: unknown, typ: Typ): string {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return convertBasicValue(value);
  }

  const obj = value as Record<string, unknown>;
  const entries: string[] = [];

  if (typ.fields) {
    for (const field of typ.fields) {
      if (field.name in obj) {
        const fieldValue = obj[field.name];
        const convertedValue = convertValueWithType(fieldValue, field.typ);
        entries.push(`${field.name}: ${convertedValue}`);
      }
    }
  } else {
    for (const [key, val] of Object.entries(obj)) {
      entries.push(`${key}: ${convertBasicValue(val)}`);
    }
  }

  return `{${entries.join(", ")}}`;
}

function convertVariant(value: unknown, typ: Typ): string {
  // Handle string values as potential unit variants
  if (typeof value === "string") {
    // Check if this string matches a unit variant case
    if (typ.cases) {
      const unitCase = typ.cases.find(
        c => (typeof c === "string" ? c : c.name) === value,
      );
      if (unitCase) {
        return value; // Return the variant case name directly
      }
    }
    // If not a unit variant, treat as regular string
    return convertBasicValue(value);
  }

  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return convertBasicValue(value);
  }

  const obj = value as Record<string, unknown>;
  const entries = Object.entries(obj);

  if (entries.length !== 1) {
    return convertBasicValue(value);
  }
  let caseName = entries[0]?.[0] || "";
  let caseValue: unknown;
  if (entries[0]) {
    [caseName, caseValue] = entries[0];
  }

  if (caseValue === null || caseValue === undefined) {
    return caseName;
  }

  // Find case type
  let caseType: Typ | undefined;
  if (typ.cases) {
    const caseInfo = typ.cases.find(c =>
      typeof c === "object" && "name" in c
        ? c.name === caseName
        : c === caseName,
    );
    if (typeof caseInfo === "object" && "typ" in caseInfo) {
      caseType = caseInfo.typ;
    }
  }

  const convertedValue = convertValueWithType(caseValue, caseType);
  return `${caseName}(${convertedValue})`;
}

function convertResult(value: unknown, typ: Typ): string {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return convertValueWithType(value, typ.ok);
  }

  const obj = value as Record<string, unknown>;

  if ("ok" in obj) {
    const okValue = obj.ok;
    if (okValue === null || okValue === undefined) {
      return "ok(null)";
    }

    const convertedOk = convertValueWithType(okValue, typ.ok);
    return typ.ok && isSimpleType(typ.ok.type)
      ? convertedOk
      : `ok(${convertedOk})`;
  }

  if ("err" in obj) {
    const errValue = obj.err;
    const convertedErr = convertValueWithType(errValue, typ.err);
    return `err(${convertedErr})`;
  }

  return convertValueWithType(value, typ.ok);
}

function convertTuple(value: unknown, typ: Typ): string {
  if (!Array.isArray(value)) {
    return convertBasicValue(value);
  }

  const items: string[] = [];

  if (typ.fields) {
    for (let i = 0; i < value.length && i < typ.fields.length; i++) {
      const elementValue = value[i];
      const elementType = typ.fields?.[i]?.typ;
      items.push(convertValueWithType(elementValue, elementType));
    }
  } else {
    items.push(...value.map(item => convertBasicValue(item)));
  }

  return `(${items.join(", ")})`;
}

function convertBasicValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "null";
  }

  switch (typeof value) {
    case "string":
      return `"${value.replace(/"/g, '\\"')}"`;
    case "number":
    case "boolean":
      return String(value);
    case "object":
      if (Array.isArray(value)) {
        const items = value.map(item => convertBasicValue(item));
        return `[${items.join(", ")}]`;
      } else {
        const entries = Object.entries(value as Record<string, unknown>).map(
          ([key, val]) => `${key}: ${convertBasicValue(val)}`,
        );
        return `{${entries.join(", ")}}`;
      }
    default:
      return JSON.stringify(value);
  }
}

function isSimpleType(type: string): boolean {
  return [
    "str",
    "string",
    "bool",
    "u8",
    "u16",
    "u32",
    "u64",
    "s8",
    "s16",
    "s32",
    "s64",
    "f32",
    "f64",
    "chr",
    "enum",
  ].includes(type);
}

export function convertPayloadToWaveArgs(payload: {
  params: Array<{ value: unknown; typ?: Typ }>;
}): string[] {
  return payload.params.map(param =>
    convertValueWithType(param.value, param.typ),
  );
}

export function convertValuesToWaveArgs(values: unknown[]): string[] {
  return values.map(value => convertBasicValue(value));
}

export function convertToWaveFormatWithType(
  value: unknown,
  parameter?: Parameter,
): string {
  return convertValueWithType(value, parameter?.typ);
}
