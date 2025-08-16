export interface Typ {
  type: string;
  fields?: Field[];
  cases?: Case[] | string[];
  inner?: Typ;
  ok?: Typ;
  err?: Typ;
  names?: string[];
}

export interface Field {
  name: string;
  typ: Typ;
}

export type TypeField = {
  name: string;
  typ: {
    type: string;
    inner?: Field["typ"];
    fields?: Field[];
    cases?: Array<string | { name: string; typ: Field["typ"] }>;
    names?: string[];
    ok?: Field["typ"];
    err?: Field["typ"];
  };
};

export interface Case {
  name: string;
  typ: Typ;
}

export interface Function {
  name: string;
  parameters: Parameter[];
  results: Result[];
}

export interface Parameter {
  type: string;
  name: string;
  typ: Typ;
}

export interface Result {
  name: string | null;
  typ: Typ;
}

export interface Export {
  name: string;
  type: string;
  functions: Function[];
}

export interface Memory {
  initial: number;
  maximum: number | null;
}

export interface Value {
  name: string;
  version: string;
}

export interface FieldProducer {
  name: string;
  values: Value[];
}

export interface Producer {
  fields: FieldProducer[];
}

export interface Metadata {
  exports: string[];
  memories: Memory[];
  producers: Producer[];
}

export interface VersionedComponentId {
  componentId?: string;
  version?: number;
}

export enum ComponentType {
  Durable = "Durable",
  Ephemeral = "Ephemeral",
}

export interface Component {
  componentVersion?: number;
  componentName?: string;
  componentSize?: number;
  componentType?: ComponentType;
  createdAt?: string;
  files?: FileStructure[];
  installedPlugins?: InstalledPlugin[];
  metadata?: Metadata;
  projectId?: string;
  componentId?: string;
  exports?: string[];
  parsedExports?: Export[];
  // versionedComponentId?: VersionedComponentId;
}

export interface FileStructure {
  key: string;
  path: string;
  permissions: string;
}

export interface InstalledPlugin {
  id: string;
  name: string;
  version: string;
  priority: number;
  parameters: unknown;
}

export interface ComponentList {
  componentName?: string;
  componentType?: string;
  versions?: Component[];
  versionList?: number[];
  componentId?: string;
}

export interface ComponentExportFunction {
  name: string;
  parameters: Parameter[];
  results: Result[];
  exportName?: string;
}

function parseType(typeStr: string): Typ {
  const trimmed = typeStr.trim();

  // Handle record types: record { field1: type1, field2: type2 }
  if (trimmed.startsWith("record ") && trimmed.includes("{")) {
    const braceStart = trimmed.indexOf("{");
    const braceEnd = trimmed.lastIndexOf("}");
    if (braceStart !== -1 && braceEnd > braceStart) {
      const fieldsStr = trimmed.substring(braceStart + 1, braceEnd).trim();

      const fields = parseRecordFields(fieldsStr);

      return {
        type: "record",
        fields,
      };
    }
  }

  // Handle enum types: enum { value1, value2, value3 }
  if (trimmed.startsWith("enum ") && trimmed.includes("{")) {
    const braceStart = trimmed.indexOf("{");
    const braceEnd = trimmed.lastIndexOf("}");
    if (braceStart !== -1 && braceEnd > braceStart) {
      const enumStr = trimmed.substring(braceStart + 1, braceEnd).trim();
      const cases = enumStr
        .split(",")
        .map(s => s.trim())
        .filter(s => s.length > 0);

      return {
        type: "enum",
        cases,
      };
    }
  }

  // Handle variant types: variant { case1, case2(type), case3 }
  if (trimmed.startsWith("variant ") && trimmed.includes("{")) {
    const braceStart = trimmed.indexOf("{");
    const braceEnd = trimmed.lastIndexOf("}");
    if (braceStart !== -1 && braceEnd > braceStart) {
      const variantStr = trimmed.substring(braceStart + 1, braceEnd).trim();
      const cases = parseVariantCases(variantStr);

      return {
        type: "variant",
        cases,
      };
    }
  }

  // Handle flags types: flags { flag1, flag2, flag3 }
  if (trimmed.startsWith("flags ") && trimmed.includes("{")) {
    const braceStart = trimmed.indexOf("{");
    const braceEnd = trimmed.lastIndexOf("}");
    if (braceStart !== -1 && braceEnd > braceStart) {
      const flagsStr = trimmed.substring(braceStart + 1, braceEnd).trim();
      const names = flagsStr
        .split(",")
        .map(s => s.trim())
        .filter(s => s.length > 0);

      return {
        type: "flags",
        names,
      };
    }
  }

  // Handle generic types with angle brackets
  if (trimmed.startsWith("handle<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(7, -1);
    return {
      type: "handle",
      inner: parseType(inner),
    };
  }

  if (trimmed.startsWith("&handle<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(8, -1);
    return {
      type: "handle",
      inner: parseType(inner),
    };
  }

  if (trimmed.startsWith("tuple<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(6, -1);
    const elements = splitTypeArguments(inner);
    return {
      type: "tuple",
      fields: elements.map((typ, i) => ({
        name: `_${i}`,
        typ: parseType(typ),
      })),
    };
  }

  if (trimmed.startsWith("list<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(5, -1);
    return {
      type: "list",
      inner: parseType(inner),
    };
  }

  if (trimmed.startsWith("option<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(7, -1);
    return {
      type: "option",
      inner: parseType(inner),
    };
  }

  if (trimmed.startsWith("result<") && trimmed.endsWith(">")) {
    const inner = trimmed.slice(7, -1);
    const parts = splitTypeArguments(inner);
    if (parts.length === 2) {
      return {
        type: "result",
        ok: parseType(parts[0]!),
        err: parseType(parts[1]!),
      };
    } else if (parts.length === 1) {
      return {
        type: "result",
        ok: parseType(parts[0]!),
      };
    }
  }

  // Handle primitive types with lowercase
  const typeMapping: Record<string, string> = {
    string: "str",
    bool: "bool",
    u8: "u8",
    u16: "u16",
    u32: "u32",
    u64: "u64",
    s8: "s8",
    s16: "s16",
    s32: "s32",
    s64: "s64",
    f32: "f32",
    f64: "f64",
    char: "chr",
    _: "unit",
  };

  return { type: typeMapping[trimmed] || trimmed };
}

// Helper function to split type arguments respecting nested brackets
function splitTypeArguments(str: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  let i = 0;

  while (i < str.length) {
    const char = str[i];

    if (char === "<" || char === "(" || char === "{") {
      depth++;
      current += char;
    } else if (char === ">" || char === ")" || char === "}") {
      depth--;
      current += char;
    } else if (char === "," && depth === 0) {
      if (current.trim()) {
        parts.push(current.trim());
      }
      current = "";
    } else {
      current += char;
    }
    i++;
  }

  if (current.trim()) {
    parts.push(current.trim());
  }

  return parts;
}

// Helper function to parse record fields
function parseRecordFields(fieldsStr: string): Field[] {
  if (!fieldsStr.trim()) return [];

  const fields: Field[] = [];
  let depth = 0;
  let current = "";
  let i = 0;

  while (i < fieldsStr.length) {
    const char = fieldsStr[i];

    if (char === "<" || char === "(" || char === "{") {
      depth++;
      current += char;
    } else if (char === ">" || char === ")" || char === "}") {
      depth--;
      current += char;
    } else if (char === "," && depth === 0) {
      if (current.trim()) {
        const field = parseRecordField(current.trim());
        if (field) fields.push(field);
      }
      current = "";
    } else {
      current += char;
    }
    i++;
  }

  if (current.trim()) {
    const field = parseRecordField(current.trim());
    if (field) fields.push(field);
  }

  return fields;
}

// Helper function to parse a single record field
function parseRecordField(fieldStr: string): Field | null {
  const colonIndex = fieldStr.indexOf(":");
  if (colonIndex === -1) return null;

  const name = fieldStr.substring(0, colonIndex).trim();
  const typeStr = fieldStr.substring(colonIndex + 1).trim();

  return {
    name,
    typ: parseType(typeStr),
  };
}

function parseParameters(paramStr: string): Parameter[] {
  if (!paramStr.trim()) return [];

  const paramStrings = splitTypeArguments(paramStr);
  const params: Parameter[] = [];

  for (const paramString of paramStrings) {
    const param = parseParameter(paramString);
    if (param) params.push(param);
  }

  return params;
}

// function parseParameter(paramStr: string): Parameter | null {
//   const colonIndex = paramStr.lastIndexOf(":");
//   if (colonIndex === -1) return null;

//   const name = paramStr.substring(0, colonIndex).trim();
//   const typeStr = paramStr.substring(colonIndex + 1).trim();

//   return {
//     name,
//     type: typeStr,
//     typ: parseType(typeStr),
//   };
// }

function parseParameter(paramStr: string): Parameter | null {
  // Find the first colon that's not inside brackets/braces
  let depth = 0;
  let colonIndex = -1;

  for (let i = 0; i < paramStr.length; i++) {
    const char = paramStr[i];

    if (char === "<" || char === "(" || char === "{") {
      depth++;
    } else if (char === ">" || char === ")" || char === "}") {
      depth--;
    } else if (char === ":" && depth === 0) {
      colonIndex = i;
      break;
    }
  }

  if (colonIndex === -1) return null;

  const name = paramStr.substring(0, colonIndex).trim();
  const typeStr = paramStr.substring(colonIndex + 1).trim();

  return {
    name,
    type: typeStr,
    typ: parseType(typeStr),
  };
}

function parseResults(resultStr: string): Result[] {
  if (!resultStr.trim()) return [];

  if (resultStr.startsWith("(") && resultStr.endsWith(")")) {
    const inner = resultStr.slice(1, -1);
    const types = splitTypeArguments(inner);
    return types.map((typeStr, i) => ({
      name: `_${i}`,
      typ: parseType(typeStr),
    }));
  }

  return [
    {
      name: null,
      typ: parseType(resultStr),
    },
  ];
}

export function parseExportString(exportStr: string): Export | null {
  try {
    const parenIndex = exportStr.indexOf("(");
    const arrowIndex = exportStr.indexOf(" -> ");

    let functionPart: string;
    let parametersPart = "";
    let resultsPart = "";

    if (parenIndex !== -1) {
      functionPart = exportStr.substring(0, parenIndex);

      let parenEndIndex: number;
      if (arrowIndex !== -1 && arrowIndex > parenIndex) {
        parenEndIndex = exportStr.lastIndexOf(")", arrowIndex);
        resultsPart = exportStr.substring(arrowIndex + 4).trim();
      } else {
        parenEndIndex = exportStr.lastIndexOf(")");
      }

      if (parenEndIndex > parenIndex) {
        parametersPart = exportStr.substring(parenIndex + 1, parenEndIndex);
      }
    } else {
      if (arrowIndex !== -1) {
        functionPart = exportStr.substring(0, arrowIndex);
        resultsPart = exportStr.substring(arrowIndex + 4).trim();
      } else {
        functionPart = exportStr;
      }
    }

    functionPart = functionPart.trim();

    let interfaceName = "";
    let functionName = functionPart;

    const braceStart = functionPart.indexOf(".{");
    if (braceStart !== -1) {
      interfaceName = functionPart.substring(0, braceStart);
      const braceEnd = functionPart.lastIndexOf("}");
      if (braceEnd > braceStart) {
        functionName = functionPart.substring(braceStart + 2, braceEnd);
      }
    }

    const parameters = parseParameters(parametersPart);
    const results = parseResults(resultsPart);

    const func: Function = {
      name: functionName,
      parameters,
      results,
    };

    return {
      name: interfaceName || functionName,
      type: "function",
      functions: [func],
    };
  } catch (error) {
    console.warn("Failed to parse export string:", exportStr, error);
    return null;
  }
}

// Helper function to parse variant cases
function parseVariantCases(casesStr: string): Case[] {
  if (!casesStr.trim()) return [];

  const cases: Case[] = [];
  let depth = 0;
  let current = "";
  let i = 0;

  while (i < casesStr.length) {
    const char = casesStr[i];

    if (char === "<" || char === "(" || char === "{") {
      depth++;
      current += char;
    } else if (char === ">" || char === ")" || char === "}") {
      depth--;
      current += char;
    } else if (char === "," && depth === 0) {
      if (current.trim()) {
        const caseItem = parseVariantCase(current.trim());
        if (caseItem) cases.push(caseItem);
      }
      current = "";
    } else {
      current += char;
    }
    i++;
  }

  if (current.trim()) {
    const caseItem = parseVariantCase(current.trim());
    if (caseItem) cases.push(caseItem);
  }

  return cases;
}

// Helper function to parse a single variant case
function parseVariantCase(caseStr: string): Case | null {
  const parenIndex = caseStr.indexOf("(");

  if (parenIndex === -1) {
    // Simple case without payload
    return {
      name: caseStr.trim(),
      typ: { type: "unit" },
    };
  }

  // Case with payload
  const name = caseStr.substring(0, parenIndex).trim();
  const parenEnd = caseStr.lastIndexOf(")");

  if (parenEnd > parenIndex) {
    const typeStr = caseStr.substring(parenIndex + 1, parenEnd).trim();
    return {
      name,
      typ: parseType(typeStr),
    };
  }

  return null;
}
