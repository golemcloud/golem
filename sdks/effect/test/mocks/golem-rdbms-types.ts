/**
 * In-memory mock for `golem:rdbms/types@1.5.0`. Pure types, so this
 * file just re-exports the same type aliases as the real binding.
 * Vitest aliases the WIT specifier to this file.
 */

export interface Uuid {
  highBits: bigint
  lowBits: bigint
}

export type IpAddress =
  | { tag: "ipv4"; val: [number, number, number, number] }
  | {
      tag: "ipv6"
      val: [number, number, number, number, number, number, number, number]
    }

export interface MacAddress {
  octets: [number, number, number, number, number, number]
}

export interface Date {
  year: number
  month: number
  day: number
}

export interface Time {
  hour: number
  minute: number
  second: number
  nanosecond: number
}

export interface Timestamp {
  date: Date
  time: Time
}

export interface Timestamptz {
  timestamp: Timestamp
  offset: number
}

export interface Timetz {
  time: Time
  offset: number
}
