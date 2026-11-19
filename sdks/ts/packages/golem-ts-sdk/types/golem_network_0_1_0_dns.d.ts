/**
 * DNS record lookups.
 * `wasi:sockets/ip-name-lookup` resolves names to addresses only. This interface queries one
 * record type of one name and returns the typed records of the answer with their TTLs.
 * Each call is one durable operation of the agent: the host records the answer, and a replay
 * returns the recorded answer, including a negative answer or an error, without a query. A
 * transient resolver failure retries under the agent's retry policy. A lookup needs the same
 * `network` grant on the name as an address lookup, and the operator must enable agent sockets.
 * The A and AAAA answers link their addresses to the queried name, so a grant on the name also
 * allows a socket connect to those addresses.
 * The host uses the upstream servers of the executor configuration, by default the system
 * resolver configuration. It does not read the hosts file and does not cache answers.
 */
declare module 'golem:network/dns@0.1.0' {
  /**
   * Queries the records of type `record-type` of `name`. The answer holds only records of that
   * type; the CNAME records of a followed alias chain are left out.
   * @throws DnsError
   */
  export function resolve(name: string, recordType: RecordType): Promise<DnsRecord[]>;
  /**
   * The record type to query.
   */
  export type RecordType = "a" | "aaaa" | "cname" | "mx" | "ns" | "ptr" | "srv" | "txt" | "caa" | "soa" | "naptr";
  export type Ipv4Address = [number, number, number, number];
  export type Ipv6Address = [number, number, number, number, number, number, number, number];
  /**
   * A mail exchange (RFC 1035).
   */
  export type MxRecord = {
    preference: number;
    exchange: string;
  };
  /**
   * A service location (RFC 2782).
   */
  export type SrvRecord = {
    priority: number;
    weight: number;
    port: number;
    target: string;
  };
  /**
   * A certification authority authorization (RFC 8659). `issuer-critical` is the high bit of
   * the flags; `value` holds the raw value octets.
   */
  export type CaaRecord = {
    issuerCritical: boolean;
    tag: string;
    value: Uint8Array;
  };
  /**
   * The start of a zone of authority (RFC 1035).
   */
  export type SoaRecord = {
    mname: string;
    rname: string;
    serial: number;
    refresh: number;
    retry: number;
    expire: number;
    minimum: number;
  };
  /**
   * A naming authority pointer (RFC 3403).
   */
  export type NaptrRecord = {
    order: number;
    preference: number;
    naptrFlags: string;
    services: string;
    regexp: string;
    replacement: string;
  };
  /**
   * The data of one record. Domain names are in ASCII presentation form without the final dot.
   */
  export type RecordData =
  {
    tag: 'a'
    val: Ipv4Address
  } |
  {
    tag: 'aaaa'
    val: Ipv6Address
  } |
  {
    tag: 'cname'
    val: string
  } |
  {
    tag: 'mx'
    val: MxRecord
  } |
  {
    tag: 'ns'
    val: string
  } |
  {
    tag: 'ptr'
    val: string
  } |
  {
    tag: 'srv'
    val: SrvRecord
  } |
  /** The character strings of the record, as raw octets. */
  {
    tag: 'txt'
    val: Uint8Array[]
  } |
  {
    tag: 'caa'
    val: CaaRecord
  } |
  {
    tag: 'soa'
    val: SoaRecord
  } |
  {
    tag: 'naptr'
    val: NaptrRecord
  };
  /**
   * One record of an answer, with its time to live in seconds as the answer gave it.
   */
  export type DnsRecord = {
    ttl: number;
    data: RecordData;
  };
  export type DnsError =
  /** No grant allows a lookup of the name. */
  {
    tag: 'access-denied'
  } |
  /** The name is not a valid domain name. */
  {
    tag: 'invalid-name'
  } |
  /** The name does not exist (`NXDOMAIN`). */
  {
    tag: 'name-not-found'
  } |
  /** The name exists but has no records of the queried type. */
  {
    tag: 'no-data'
  } |
  /**
   * The lookup can succeed later: the servers timed out, were unreachable or answered
   * `SERVFAIL`.
   */
  {
    tag: 'temporary-resolver-failure'
  } |
  /**
   * The servers refused the query or failed in a way that a retry does not repair, or the
   * operator did not enable agent sockets.
   */
  {
    tag: 'permanent-resolver-failure'
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
