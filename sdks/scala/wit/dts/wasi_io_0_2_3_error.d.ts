declare module 'wasi:io/error@0.2.3' {
  export class Error {
    /**
     * Returns a string that is suitable to assist humans in debugging
     * this error.
     * WARNING: The returned string should not be consumed mechanically!
     * It may change across platforms, hosts, or other implementation
     * details. Parsing this string is a major platform-compatibility
     * hazard.
     */
    toDebugString(): string;
  }
}
