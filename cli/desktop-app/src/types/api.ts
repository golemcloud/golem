export type MethodPattern =
  | "GET"
  | "CONNECT"
  | "POST"
  | "DELETE"
  | "PUT"
  | "PATCH"
  | "OPTIONS"
  | "TRACE"
  | "HEAD";
export type GatewayBindingType = "default" | "file-server" | "cors-preflight";
