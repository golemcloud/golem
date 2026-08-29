declare module 'tool-middleware-guest' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemTool010Common from 'golem:tool/common@0.1.0';
  /**
   * Interface exported by components that provide tool middleware.
   */
  export namespace toolMiddlewareGuest {
    /**
     * Enumerate every middleware definition exported by this component.
     * @throws ToolError
     */
    export function discoverToolMiddlewares(): ToolMiddleware[];
    /**
     * Look up one middleware definition by canonical name.
     * @throws ToolError
     */
    export function getToolMiddleware(name: string): ToolMiddleware;
    /**
     * Invoke one middleware layer around its runtime-bound next inner layer.
     * @throws ToolError
     */
    export function invokeToolMiddleware(middlewareName: string, toolName: string, toolMetadata: Tool, commandPath: string[], input: TypedSchemaValue, stdin: AsyncIterable<number> | undefined, principal: Principal, wrapped: UnderlyingTool): Promise<InvocationResult>;
    export type InvocationResult = golemTool010Common.InvocationResult;
    export type Tool = golemTool010Common.Tool;
    export type ToolError = golemTool010Common.ToolError;
    export type ToolMiddleware = golemTool010Common.ToolMiddleware;
    export type UnderlyingTool = golemTool010Common.UnderlyingTool;
    export type Principal = golemAgent200Common.Principal;
    export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
}
