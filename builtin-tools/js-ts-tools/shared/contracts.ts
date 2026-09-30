import { z } from 'zod/v4';

export const NPM_VERSION = '10.9.9';
export const TYPESCRIPT_VERSION = '5.9.2';
export const DEFAULT_CWD = '/workspace';
export const DEFAULT_REGISTRY = 'https://registry.npmjs.org/';
export const DEFAULT_MAX_OUTPUT_BYTES = 4_194_304;
const MAX_OUTPUT_BYTES = 16_777_216;

export const ToolExecutionResultSchema = z.object({
  exitCode: z.number().int(),
  version: z.string(),
  overflowed: z.boolean(),
  stdout: z.string(),
  stderr: z.string(),
});

export type ToolExecutionResult = z.infer<typeof ToolExecutionResultSchema>;

export function normalizedRegistry(value: string): string {
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('registry must use HTTP or HTTPS');
  }
  if (url.username || url.password) {
    throw new Error('registry must not contain credentials');
  }
  url.hash = '';
  url.search = '';
  if (!url.pathname.endsWith('/')) url.pathname += '/';
  return url.toString();
}

export function validatedCwd(value: string): string {
  if (!value.startsWith('/')) throw new Error('cwd must be an absolute path');
  return value.replace(/\/+$/, '') || '/';
}

export function validatedOutputLimit(value: number): number {
  if (!Number.isInteger(value) || value < 1 || value > MAX_OUTPUT_BYTES) {
    throw new Error(`max-output-bytes must be an integer between 1 and ${MAX_OUTPUT_BYTES}`);
  }
  return value;
}

export function invocationEnvironment(cwdValue: string, registryValue: string) {
  const cwd = validatedCwd(cwdValue);
  const registry = normalizedRegistry(registryValue);
  return {
    HOME: `${cwd}/.golem-home`,
    NODE: process.execPath,
    NPM: '/toolchain/npm/node_modules/npm/bin/npm-cli.js',
    NPM_CONFIG_AUDIT: 'false',
    NPM_CONFIG_CACHE: `${cwd}/.golem-npm-cache`,
    NPM_CONFIG_FUND: 'false',
    NPM_CONFIG_PREFIX: `${cwd}/.golem-npm-prefix`,
    NPM_CONFIG_REGISTRY: registry,
    NPM_CONFIG_UPDATE_NOTIFIER: 'false',
    PATH: `${cwd}/node_modules/.bin:/usr/local/bin:/usr/bin:/bin`,
  };
}
