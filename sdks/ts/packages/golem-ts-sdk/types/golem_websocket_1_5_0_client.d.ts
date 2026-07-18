declare module 'golem:websocket/client@1.5.0' {
  export class WebsocketConnection {
    /**
     * Connect to a WebSocket server at the given URL (ws:// or wss://)
     * Optional headers for auth, subprotocols, etc.
     * @throws Error
     */
    static connect(url: string, headers: [string, string][] | undefined): WebsocketConnection;
    /**
     * Send a message (text or binary)
     * @throws Error
     */
    send(message: Message): void;
    /**
     * Receive the next message (blocks until available)
     * @throws Error
     */
    receive(): Promise<Message>;
    /**
     * Receive the next message with a timeout in milliseconds.
     * Returns none if the timeout expires before a message arrives.
     * @throws Error
     */
    receiveWithTimeout(timeoutMs: bigint): Promise<Message | undefined>;
    /**
     * Send a close frame with optional code and reason
     * @throws Error
     */
    close(code: number | undefined, reason: string | undefined): void;
  }
  export type CloseInfo = {
    code: number;
    reason: string;
  };
  export type Error = 
  {
    tag: 'connection-failure'
    val: string
  } |
  {
    tag: 'send-failure'
    val: string
  } |
  {
    tag: 'receive-failure'
    val: string
  } |
  {
    tag: 'protocol-error'
    val: string
  } |
  {
    tag: 'closed'
    val: CloseInfo | undefined
  } |
  {
    tag: 'other'
    val: string
  };
  /**
   * A WebSocket message — text or binary
   */
  export type Message = 
  {
    tag: 'text'
    val: string
  } |
  {
    tag: 'binary'
    val: Uint8Array
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
