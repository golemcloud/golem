import { Case, FileStructure, Typ } from "@/types/component";
import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";
import { HttpApiDefinition } from "@/types/golemManifest.ts";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatRelativeTime(dateString: string | number | Date) {
  const date = new Date(dateString).getTime();
  const now = new Date().getTime();
  const diffInSeconds = Math.floor((now - date) / 1000);

  const units = [
    { name: "year", seconds: 60 * 60 * 24 * 365 },
    { name: "month", seconds: 60 * 60 * 24 * 30 },
    { name: "week", seconds: 60 * 60 * 24 * 7 },
    { name: "day", seconds: 60 * 60 * 24 },
    { name: "hour", seconds: 60 * 60 },
    { name: "minute", seconds: 60 },
    { name: "second", seconds: 1 },
  ];

  for (const unit of units) {
    if (diffInSeconds >= unit.seconds) {
      const value = Math.floor(diffInSeconds / unit.seconds);
      return `${value} ${unit.name}${value > 1 ? "s" : ""} ago`;
    }
  }

  return "just now";
}

export const sanitizeInput = (input: string): string => {
  return input.replace(/[“”\u201c\u201d]/g, '"').replace(/[‘’]/g, "'");
};

export function formatTimestampInDateTimeFormat(timestamp: string) {
  const date = new Date(timestamp);

  // Get date components
  const month = String(date.getMonth() + 1).padStart(2, "0"); // Months are zero-indexed
  const day = String(date.getDate()).padStart(2, "0");
  const year = date.getFullYear();

  // Get time components
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const seconds = String(date.getSeconds()).padStart(2, "0");

  // Combine into MM/DD/YYYY HH:MM:SS format
  return `${month}/${day}/${year} ${hours}:${minutes}:${seconds}`;
}

/// compare semver version
export const compareSemver = (version1: string, version2: string) => {
  const semverRegex =
    /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

  if (!semverRegex.test(version1) || !semverRegex.test(version2)) {
    throw new Error("Invalid semver version format");
  }

  const v1Parts = version1.split(".");
  const v2Parts = version2.split(".");

  // Compare major version
  const major1 = parseInt(v1Parts[0] || "0", 10);
  const major2 = parseInt(v2Parts[0] || "0", 10);
  if (major1 !== major2) return major1 > major2;

  const minor1 = parseInt(v1Parts[1] || "0", 10);
  const minor2 = parseInt(v2Parts[1] || "0", 10);
  if (minor1 !== minor2) return minor1 > minor2;

  // Compare patch version
  const patch1 = parseInt(v1Parts[2] || "0", 10);
  const patch2 = parseInt(v2Parts[2] || "0", 10);
  return patch1 > patch2;

  return false;
};

/// Remove the duplicate api and keep the latest one by comparing the semver version
export const removeDuplicateApis = (data: HttpApiDefinition[]) => {
  const uniqueEntries = {} as Record<
    string,
    HttpApiDefinition & { count?: number }
  >;

  data.forEach(item => {
    if (!item.id) return; // Skip items without id

    if (!uniqueEntries[item.id]) {
      uniqueEntries[item.id] = item;
    } else {
      // check semver for latest version
      const uniqueEntriesVersion = uniqueEntries[item.id]?.version;
      const count = (uniqueEntries[item.id]?.count || 1) + 1;
      const itemVersion = item.version;
      if (compareSemver(itemVersion, uniqueEntriesVersion!)) {
        uniqueEntries[item.id] = { ...item, count: count + 1 };
      } else {
        uniqueEntries[item.id]!.count = count;
      }
    }
  });
  return Object.values(uniqueEntries);
};

export const parseErrorMessage = (error: string): string => {
  const patterns = [
    /([^"]*Component already exists: [a-f0-9-]+)/,
    /(?<=: ).*?(?=\s\(occurred)/, // Extract message before "(occurred"
    /(?<=error: ).*?(?=:|$)/i, // Extract message after "error: "
    /Invalid value for the key [^:]+/, // Extract key-specific error
  ];

  for (const pattern of patterns) {
    const match = error.match(pattern);
    if (match) {
      return match[0];
    }
  }
  return "An unknown error occurred.";
};

interface FileNode {
  name: string;
  type: "file" | "folder";
  children?: FileNode[];
  permissions?: string;
}

export function parseFileStructure(data: FileStructure[]): FileNode {
  const root: FileNode = { name: "root", type: "folder", children: [] };

  data.forEach(item => {
    const parts = item.path.split("/").filter(Boolean);
    let currentNode = root;

    parts.forEach((part: string, index: number) => {
      if (index === parts.length - 1) {
        currentNode.children?.push({
          name: part,
          type: "file",
          permissions: item.permissions,
        });
      } else {
        let folderNode = currentNode.children?.find(
          child => child.name === part && child.type === "folder",
        );
        if (!folderNode) {
          folderNode = { name: part, type: "folder", children: [] };
          currentNode.children?.push(folderNode);
        }
        currentNode = folderNode;
      }
    });
  });

  return root;
}

/**
 * Returns the short name and full multiline representation of a WIT-like type.
 */
export function parseTypeForTooltip(typ: Typ | undefined): {
  short: string;
  full: string;
} {
  if (!typ) {
    return { short: "null", full: "null" };
  }

  switch (typ.type) {
    case "Bool":
      return { short: "bool", full: "bool" };
    case "S8":
    case "S16":
    case "S32":
    case "S64":
      return { short: `i${typ.type.slice(1)}`, full: `i${typ.type.slice(1)}` };
    case "U8":
    case "U16":
    case "U32":
    case "U64":
      return { short: `u${typ.type.slice(1)}`, full: `u${typ.type.slice(1)}` };
    case "F32":
    case "F64":
      return { short: typ.type.toLowerCase(), full: typ.type.toLowerCase() };
    case "Char":
      return { short: "char", full: "char" };
    case "Str":
      return { short: "string", full: "String" };
    case "List": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `list<${inner.short}>`,
        full: `list<${inner.full}>`,
      };
    }
    case "Option": {
      const inner = parseTypeForTooltip(typ.inner);
      return {
        short: `option<${inner.short}>`,
        full: `Option<${inner.full}>`,
      };
    }
    case "Result": {
      const okParsed = parseTypeForTooltip(typ.ok);
      const errParsed = parseTypeForTooltip(typ.err);
      return {
        short: `result<${okParsed.short}, ${errParsed.short}>`,
        full: `Result<${okParsed.full}, ${errParsed.full}>`,
      };
    }
    case "Tuple": {
      const elements = (typ.fields || []).map(element =>
        parseTypeForTooltip(element.typ),
      );
      return {
        short: `tuple<${elements.map(e => e.short).join(", ")}>`,
        full: `(${elements.map(e => e.full).join(", ")})`,
      };
    }
    case "Record": {
      const fields = (typ.fields || []).map(field => {
        const parsed = parseTypeForTooltip(field.typ);
        return `"${field.name}": ${parsed.full}`;
      });
      return {
        short: "record",
        full: `{\n  ${fields.join(",\n  ")}\n}`,
      };
    }
    case "Variant": {
      const cases = ((typ.cases as Case[]) || []).map(c => {
        const parsed = parseTypeForTooltip(c.typ);
        return `${c.name.charAt(0).toUpperCase() + c.name.slice(1)}(${parsed.full})`;
      });
      return {
        short: "variant",
        full: `enum {\n  ${cases.join(",\n  ")}\n}`,
      };
    }
    case "Enum": {
      const cases = ((typ.cases as string[]) || []).map(
        c => c.charAt(0).toUpperCase() + c.slice(1),
      );
      return {
        short: "enum",
        full: `enum (\n  ${cases.join(",\n  ")}\n)`,
      };
    }
    default:
      return { short: "unknown", full: "unknown" };
  }
}
