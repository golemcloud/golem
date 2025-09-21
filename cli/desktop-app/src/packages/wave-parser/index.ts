/**
 * TypeScript WAVE (WebAssembly Value Encoding) Parser
 *
 * This module provides functionality to parse WAVE format strings into plain JavaScript objects.
 * WAVE is used by Golem and other WebAssembly systems to encode values in a text format.
 *
 * @example
 * ```typescript
 * import { parseWave } from './index';
 *
 * // Parse primitive values
 * const bool = parseWave('true'); // true
 * const num = parseWave('42'); // 42
 * const str = parseWave('"hello"'); // "hello"
 *
 * // Parse complex structures
 * const list = parseWave('[1, 2, 3]'); // [1, 2, 3]
 * const record = parseWave('{ name: "Alice", age: 30 }'); // { name: "Alice", age: 30 }
 * const option = parseWave('some(42)'); // 42
 * const result = parseWave('ok("success")'); // { ok: "success" }
 * ```
 */

export { parseWave } from "./wave-parser";

// Re-export for convenience
import { parseWave } from "./wave-parser";

export default parseWave;
