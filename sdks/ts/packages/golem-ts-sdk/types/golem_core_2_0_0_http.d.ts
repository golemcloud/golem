declare module 'golem:core/http@2.0.0' {
  export type HttpRouteMode = "durable-streams" | "plain" | "sse" | "web-socket";
  export type DurableStreamSlotSource =
  {
    tag: 'input'
    val: string
  } |
  {
    tag: 'output'
    val: string
  };
  export type DurableStreamSlotOptions = {
    source: DurableStreamSlotSource;
    name?: string;
    contentType?: string;
  };
  export type DurableStreamRouteLoadOptions = {
    maxConcurrentReadersPerStream?: number;
    maxAppendRequestsPerSecondPerStream?: number;
  };
  export type DurableStreamRouteOptions = {
    slots: DurableStreamSlotOptions[];
    load?: DurableStreamRouteLoadOptions;
  };
}
