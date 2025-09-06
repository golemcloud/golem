declare module 'golem:rdbms/types@0.0.1' {
  export type Uuid = {
    highBits: bigint;
    lowBits: bigint;
  };
  export type IpAddress = {
    tag: 'ipv4'
    val: [number, number, number, number]
  } |
  {
    tag: 'ipv6'
    val: [number, number, number, number, number, number, number, number]
  };
  export type MacAddress = {
    octets: [number, number, number, number, number, number];
  };
  export type Date = {
    year: number;
    month: number;
    day: number;
  };
  export type Time = {
    hour: number;
    minute: number;
    second: number;
    nanosecond: number;
  };
  export type Timestamp = {
    date: Date;
    time: Time;
  };
  export type Timestamptz = {
    timestamp: Timestamp;
    offset: number;
  };
  export type Timetz = {
    time: Time;
    offset: number;
  };
}
