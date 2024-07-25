declare module "golem:rpc/types@0.1.0" {
  import type { Pollable } from "wasi:io/poll@0.2.0";
  export type NodeIndex = number;
  export interface Uri {
    value: string,
  }
  export type WitNode = WitNodeRecordValue | WitNodeVariantValue | WitNodeEnumValue | WitNodeFlagsValue | WitNodeTupleValue | WitNodeListValue | WitNodeOptionValue | WitNodeResultValue | WitNodePrimU8 | WitNodePrimU16 | WitNodePrimU32 | WitNodePrimU64 | WitNodePrimS8 | WitNodePrimS16 | WitNodePrimS32 | WitNodePrimS64 | WitNodePrimFloat32 | WitNodePrimFloat64 | WitNodePrimChar | WitNodePrimBool | WitNodePrimString | WitNodeHandle;
  export interface WitNodeRecordValue {
    tag: 'record-value',
    val: Int32Array,
  }
  export interface WitNodeVariantValue {
    tag: 'variant-value',
    val: [number, NodeIndex | undefined],
  }
  export interface WitNodeEnumValue {
    tag: 'enum-value',
    val: number,
  }
  export interface WitNodeFlagsValue {
    tag: 'flags-value',
    val: boolean[],
  }
  export interface WitNodeTupleValue {
    tag: 'tuple-value',
    val: Int32Array,
  }
  export interface WitNodeListValue {
    tag: 'list-value',
    val: Int32Array,
  }
  export interface WitNodeOptionValue {
    tag: 'option-value',
    val: NodeIndex | undefined,
  }
  export interface WitNodeResultValue {
    tag: 'result-value',
    val: Result<NodeIndex | undefined, NodeIndex | undefined>,
  }
  export interface WitNodePrimU8 {
    tag: 'prim-u8',
    val: number,
  }
  export interface WitNodePrimU16 {
    tag: 'prim-u16',
    val: number,
  }
  export interface WitNodePrimU32 {
    tag: 'prim-u32',
    val: number,
  }
  export interface WitNodePrimU64 {
    tag: 'prim-u64',
    val: bigint,
  }
  export interface WitNodePrimS8 {
    tag: 'prim-s8',
    val: number,
  }
  export interface WitNodePrimS16 {
    tag: 'prim-s16',
    val: number,
  }
  export interface WitNodePrimS32 {
    tag: 'prim-s32',
    val: number,
  }
  export interface WitNodePrimS64 {
    tag: 'prim-s64',
    val: bigint,
  }
  export interface WitNodePrimFloat32 {
    tag: 'prim-float32',
    val: number,
  }
  export interface WitNodePrimFloat64 {
    tag: 'prim-float64',
    val: number,
  }
  export interface WitNodePrimChar {
    tag: 'prim-char',
    val: string,
  }
  export interface WitNodePrimBool {
    tag: 'prim-bool',
    val: boolean,
  }
  export interface WitNodePrimString {
    tag: 'prim-string',
    val: string,
  }
  export interface WitNodeHandle {
    tag: 'handle',
    val: [Uri, bigint],
  }
  export interface WitValue {
    nodes: WitNode[],
  }
  export type RpcError = RpcErrorProtocolError | RpcErrorDenied | RpcErrorNotFound | RpcErrorRemoteInternalError;
  export interface RpcErrorProtocolError {
    tag: 'protocol-error',
    val: string,
  }
  export interface RpcErrorDenied {
    tag: 'denied',
    val: string,
  }
  export interface RpcErrorNotFound {
    tag: 'not-found',
    val: string,
  }
  export interface RpcErrorRemoteInternalError {
    tag: 'remote-internal-error',
    val: string,
  }
  export class WasmRpc {
    constructor(location: Uri)
    invokeAndAwait(functionName: string, functionParams: WitValue[]): Result<WitValue, RpcError>;
    invoke(functionName: string, functionParams: WitValue[]): Result<void, RpcError>;
    asyncInvokeAndAwait(functionName: string, functionParams: WitValue[]): FutureInvokeResult;
  }
  export class FutureInvokeResult {
    subscribe(): Pollable;
    get(): Result<WitValue, RpcError> | undefined;
  }
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
