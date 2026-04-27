/**
 * Runtime mock for `golem:core/types@1.5.0`. Real Golem expects UUIDs in
 * canonical 8-4-4-4-12 hex form; we replicate that strictly enough to
 * round-trip through tests.
 */

export type Uuid = { highBits: bigint; lowBits: bigint }

const HEX_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/

export const parseUuid = (uuid: string): Uuid => {
  if (!HEX_RE.test(uuid)) {
    // Real host throws a string per the WIT binding.
    throw `invalid uuid: ${uuid}`
  }
  const hex = uuid.replace(/-/g, "")
  const high = BigInt("0x" + hex.slice(0, 16))
  const low = BigInt("0x" + hex.slice(16))
  return { highBits: high, lowBits: low }
}

const pad = (s: string, n: number): string => s.padStart(n, "0")

export const uuidToString = (uuid: Uuid): string => {
  const high = pad(uuid.highBits.toString(16), 16)
  const low = pad(uuid.lowBits.toString(16), 16)
  const hex = high + low
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}
