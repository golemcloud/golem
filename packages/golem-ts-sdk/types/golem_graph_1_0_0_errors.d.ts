/**
 * Error handling unified across all graph database providers
 */
declare module 'golem:graph/errors@1.0.0' {
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  export type ElementId = golemGraph100Types.ElementId;
  /**
   * Comprehensive error types that can represent failures across different graph databases
   */
  export type GraphError = {
    tag: 'unsupported-operation'
    val: string
  } |
  {
    tag: 'connection-failed'
    val: string
  } |
  {
    tag: 'authentication-failed'
    val: string
  } |
  {
    tag: 'authorization-failed'
    val: string
  } |
  {
    tag: 'element-not-found'
    val: ElementId
  } |
  {
    tag: 'duplicate-element'
    val: ElementId
  } |
  {
    tag: 'schema-violation'
    val: string
  } |
  {
    tag: 'constraint-violation'
    val: string
  } |
  {
    tag: 'invalid-property-type'
    val: string
  } |
  {
    tag: 'invalid-query'
    val: string
  } |
  {
    tag: 'transaction-failed'
    val: string
  } |
  {
    tag: 'transaction-conflict'
  } |
  {
    tag: 'transaction-timeout'
  } |
  {
    tag: 'deadlock-detected'
  } |
  {
    tag: 'timeout'
  } |
  {
    tag: 'resource-exhausted'
    val: string
  } |
  {
    tag: 'internal-error'
    val: string
  } |
  {
    tag: 'service-unavailable'
    val: string
  };
}
