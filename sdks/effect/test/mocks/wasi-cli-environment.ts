/**
 * Runtime mock for `wasi:cli/environment@0.2.3`.
 *
 * Tests that exercise the snapshot-load path can populate the env via
 * {@link __setEnvironment} (e.g. to control `GOLEM_AGENT_ID`).
 */

let env: Array<[string, string]> = []

export const getEnvironment = (): Array<[string, string]> => env

export const getArguments = (): Array<string> => []

export const initialCwd = (): string | undefined => undefined

export const __setEnvironment = (entries: Array<[string, string]>): void => {
  env = entries
}

export const __resetEnvironment = (): void => {
  env = []
}
