/**
 * Introspection of the permission kernel's registered resource classes and
 * verbs, and validation of individual grants against the kernel's schema.
 */
declare module 'golem:permissions/kernel-introspection@0.1.0' {
  import * as golemPermissions010Types from 'golem:permissions/types@0.1.0';
  /**
   * List every resource class the kernel knows about, with their
   * permitted verbs.
   */
  export function listModules(): KernelModule[];
  /**
   * Validate that `g` references a known class, owner, recipient, verb,
   * and resource-id pattern accepted by the kernel.
   * @throws PermissionError
   */
  export function validateGrant(g: PatternGrant): void;
  export type PatternGrant = golemPermissions010Types.PatternGrant;
  export type PermissionError = golemPermissions010Types.PermissionError;
  export type KernelModule = {
    className: string;
    verbs: string[];
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
