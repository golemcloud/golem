import { JSONPath } from "jsonpath-plus";
import { z } from "zod";

const ResultJsonAssertionSchema = z.object({
  path: z.string(),
  equals: z.unknown().optional(),
  contains: z.string().optional(),
});

export const ExpectSchema = z.object({
  exit_code: z.number().optional(),
  stdout_contains: z.string().optional(),
  stdout_not_contains: z.string().optional(),
  stdout_matches: z.string().optional(),
  status: z.number().optional(),
  body_contains: z.string().optional(),
  body_matches: z.string().optional(),
  result_json: z.array(ResultJsonAssertionSchema).optional(),
});

export type ExpectSpec = z.infer<typeof ExpectSchema>;

export interface AssertionContext {
  stdout: string;
  stderr: string;
  exitCode: number | null;
  body?: string;
  status?: number;
  resultJson?: unknown;
}

export interface AssertionResult {
  assertion: string;
  passed: boolean;
  message: string;
}

export function evaluate(
  context: AssertionContext,
  expect: ExpectSpec,
): AssertionResult[] {
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
        : `body does not contain "${expect.body_contains}"`,
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
        : `body does not match /${expect.body_matches}/`,
    });
  }

  if (expect.result_json && expect.result_json.length > 0) {
    for (const jsonAssert of expect.result_json) {
      const pathResults = JSONPath({
        path: jsonAssert.path,
        json: context.resultJson as object,
      });

      if (jsonAssert.equals !== undefined) {
        const passed =
          pathResults.length > 0 &&
          JSON.stringify(pathResults[0]) === JSON.stringify(jsonAssert.equals);
        results.push({
          assertion: `result_json[${jsonAssert.path}].equals`,
          passed,
          message: passed
            ? `${jsonAssert.path} equals ${JSON.stringify(jsonAssert.equals)}`
            : `${jsonAssert.path} expected ${JSON.stringify(jsonAssert.equals)}, got ${JSON.stringify(pathResults[0])}`,
        });
      }

      if (jsonAssert.contains !== undefined) {
        const value = pathResults.length > 0 ? String(pathResults[0]) : "";
        const passed = value.includes(jsonAssert.contains);
        results.push({
          assertion: `result_json[${jsonAssert.path}].contains`,
          passed,
          message: passed
            ? `${jsonAssert.path} contains "${jsonAssert.contains}"`
            : `${jsonAssert.path} does not contain "${jsonAssert.contains}" (got "${value}")`,
        });
      }
    }
  }

  return results;
}
