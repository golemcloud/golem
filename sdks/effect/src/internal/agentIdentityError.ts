/** The host rejected construction of an agent identity. @since 1.6.0 @category errors */
export class AgentIdentityError {
  readonly _tag = "AgentIdentityError"
  constructor(readonly cause: unknown) {}
}
