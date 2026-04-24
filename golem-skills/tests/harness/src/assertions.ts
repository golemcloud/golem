import { JSONPath } from "jsonpath-plus";
import { z } from "zod";

function deepEquals(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false;
    return a.every((val, i) => deepEquals(val, b[i]));
  }
  const keysA = Object.keys(a as Record<string, unknown>);
  const keysB = Object.keys(b as Record<string, unknown>);
  if (keysA.length !== keysB.length) return false;
  return keysA.every(
    (key) =>
      key in (b as Record<string, unknown>) &&
      deepEquals((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]),
  );
}

const ResultJsonAssertionSchema = z.object({
  path: z.string(),
  equals: z.unknown().optional(),
  equals_unordered: z.unknown().optional(),
  contains: z.string().optional(),
});

function validateRegexPattern(pattern: string, field: string, ctx: z.RefinementCtx): void {
  try {
    new RegExp(pattern);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: [field],
      message: `invalid JavaScript regular expression: ${message}`,
    });
  }
}

export const ExpectSchema = z
  .object({
    exit_code: z.number().optional(),
    stdout_contains: z.string().optional(),
    stdout_not_contains: z.string().optional(),
    stdout_matches: z.string().optional(),
    status: z.number().optional(),
    body_contains: z.string().optional(),
    body_matches: z.string().optional(),
    header_contains: z.record(z.string()).optional(),
    body_json: z.array(ResultJsonAssertionSchema).optional(),
    result_json: z.array(ResultJsonAssertionSchema).optional(),
  })
  .superRefine((expect, ctx) => {
    if (expect.stdout_matches !== undefined) {
      validateRegexPattern(expect.stdout_matches, "stdout_matches", ctx);
    }

    if (expect.body_matches !== undefined) {
      validateRegexPattern(expect.body_matches, "body_matches", ctx);
    }
  });

export type ExpectSpec = z.infer<typeof ExpectSchema>;

export interface AssertionContext {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  body?: string;
  status?: number;
  headers?: Record<string, string>;
  resultJson?: unknown;
}

export interface AssertionResult {
  assertion: string;
  passed: boolean;
  message: string;
}

function previewText(text: string, maxChars = 600): string {
  if (text.length <= maxChars) return text;
  return `${text.slice(0, maxChars)}...`;
}

function previewValue(value: unknown, maxChars = 600): string {
  if (typeof value === "string") {
    return previewText(value, maxChars);
  }

  const serialized = JSON.stringify(value);
  return serialized === undefined ? String(value) : previewText(serialized, maxChars);
}

function formatResultJsonContext(resultJson: unknown): string {
  return `; result_json=${previewValue(resultJson)}`;
}

export function evaluate(context: AssertionContext, expect: ExpectSpec): AssertionResult[] {
  const results: AssertionResult[] = [];

  if (expect.exit_code !== undefined) {
    results.push({
      assertion: "exit_code",
      passed: context.exitCode === expect.exit_code,
      message:
        context.exitCode === expect.exit_code
          ? `exit code is ${expect.exit_code}`
          : `expected exit code ${expect.exit_code}, got ${context.exitCode}`,
    });
  }

  if (expect.stdout_contains !== undefined) {
    const passed = context.stdout.includes(expect.stdout_contains);
    results.push({
      assertion: "stdout_contains",
      passed,
      message: passed
        ? `stdout contains "${expect.stdout_contains}"`
        : `stdout does not contain "${expect.stdout_contains}"`,
    });
  }

  if (expect.stdout_not_contains !== undefined) {
    const passed = !context.stdout.includes(expect.stdout_not_contains);
    results.push({
      assertion: "stdout_not_contains",
      passed,
      message: passed
        ? `stdout does not contain "${expect.stdout_not_contains}"`
        : `stdout contains "${expect.stdout_not_contains}" (should not)`,
    });
  }

  if (expect.stdout_matches !== undefined) {
    const regex = new RegExp(expect.stdout_matches);
    const passed = regex.test(context.stdout);
    results.push({
      assertion: "stdout_matches",
      passed,
      message: passed
        ? `stdout matches /${expect.stdout_matches}/`
        : `stdout does not match /${expect.stdout_matches}/`,
    });
  }

  if (expect.status !== undefined) {
    const passed = context.status === expect.status;
    results.push({
      assertion: "status",
      passed,
      message: passed
        ? `status is ${expect.status}`
        : `expected status ${expect.status}, got ${context.status}`,
    });
  }

  if (expect.body_contains !== undefined) {
    const body = context.body ?? "";
    const passed = body.includes(expect.body_contains);
    results.push({
      assertion: "body_contains",
      passed,
      message: passed
        ? `body contains "${expect.body_contains}"`
        : `body does not contain "${expect.body_contains}"; received ${JSON.stringify(previewText(body))}`,
    });
  }

  if (expect.body_matches !== undefined) {
    const body = context.body ?? "";
    const regex = new RegExp(expect.body_matches);
    const passed = regex.test(body);
    results.push({
      assertion: "body_matches",
      passed,
      message: passed
        ? `body matches /${expect.body_matches}/`
        : `body does not match /${expect.body_matches}/; received ${JSON.stringify(previewText(body))}`,
    });
  }

  if (expect.header_contains !== undefined) {
    for (const [name, expected] of Object.entries(expect.header_contains)) {
      const actual = context.headers?.[name.toLowerCase()];
      const passed = actual !== undefined && actual.includes(expected);
      results.push({
        assertion: `header_contains[${name}]`,
        passed,
        message: passed
          ? `header "${name}" contains "${expected}"`
          : `header "${name}" expected to contain "${expected}", got ${actual === undefined ? "(missing)" : JSON.stringify(actual)}`,
      });
    }
  }

  if (expect.body_json && expect.body_json.length > 0) {
    const body = context.body ?? "";
    let parsedBody: unknown;
    try {
      parsedBody = JSON.parse(body);
    } catch {
      results.push({
        assertion: "body_json",
        passed: false,
        message: `body is not valid JSON: ${previewText(body)}`,
      });
      parsedBody = undefined;
    }
    if (parsedBody !== undefined) {
      evaluateJsonAssertions(parsedBody, expect.body_json, "body_json", results);
    }
  }

  if (expect.result_json && expect.result_json.length > 0) {
    evaluateJsonAssertions(context.resultJson, expect.result_json, "result_json", results);
  }

  return results;
}

function evaluateJsonAssertions(
  json: unknown,
  assertions: z.infer<typeof ResultJsonAssertionSchema>[],
  label: string,
  results: AssertionResult[],
): void {
  for (const jsonAssert of assertions) {
    const rawPathResults = JSONPath({
      path: jsonAssert.path,
      json: json as object,
    });
    // JSONPath returns undefined for falsy root values (false, 0, null, "").
    // When querying "$" and the result is undefined, fall back to wrapping the
    // root value itself so that scalar assertions work for all values.
    const pathResults = Array.isArray(rawPathResults)
      ? rawPathResults
      : rawPathResults === undefined
        ? jsonAssert.path === "$" && json !== undefined
          ? [json]
          : []
        : [rawPathResults];

    if (jsonAssert.equals !== undefined) {
      const passed =
        pathResults.length > 0 &&
        JSON.stringify(pathResults[0]) === JSON.stringify(jsonAssert.equals);
      results.push({
        assertion: `${label}[${jsonAssert.path}].equals`,
        passed,
        message: passed
          ? `${jsonAssert.path} equals ${JSON.stringify(jsonAssert.equals)}`
          : `${jsonAssert.path} expected ${JSON.stringify(jsonAssert.equals)}, got ${JSON.stringify(pathResults[0])}${formatResultJsonContext(json)}`,
      });
    }

    if (jsonAssert.equals_unordered !== undefined) {
      const actual = pathResults.length > 0 ? pathResults[0] : undefined;
      const expected = jsonAssert.equals_unordered;
      const passed =
        Array.isArray(actual) &&
        Array.isArray(expected) &&
        actual.length === expected.length &&
        expected.every((exp: unknown) => actual.some((act: unknown) => deepEquals(act, exp)));
      results.push({
        assertion: `${label}[${jsonAssert.path}].equals_unordered`,
        passed,
        message: passed
          ? `${jsonAssert.path} equals (unordered) ${JSON.stringify(expected)}`
          : `${jsonAssert.path} expected (unordered) ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}${formatResultJsonContext(json)}`,
      });
    }

    if (jsonAssert.contains !== undefined) {
      const value = pathResults.length > 0 ? String(pathResults[0]) : "";
      const passed = value.includes(jsonAssert.contains);
      results.push({
        assertion: `${label}[${jsonAssert.path}].contains`,
        passed,
        message: passed
          ? `${jsonAssert.path} contains "${jsonAssert.contains}"`
          : `${jsonAssert.path} does not contain "${jsonAssert.contains}" (got "${value}")${formatResultJsonContext(json)}`,
      });
    }
  }
}
