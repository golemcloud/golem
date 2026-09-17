import { Effect, Exit } from "effect"

const pending = new WeakSet<object>()

export function assertCapabilityReady(handle: object): void {
  if (pending.has(handle)) throw new Error("capability validation is still in progress")
}

/** One conversion owns its rollback actions, including across Effect suspension. */
export class CapabilityTransaction {
  private readonly rollbacks: Array<() => void> = []
  private readonly handles: Array<object> = []

  lock(handle: object): void {
    if (pending.has(handle)) throw new Error("capability is already being validated")
    pending.add(handle)
    this.handles.push(handle)
  }

  register(rollback: () => void): void {
    this.rollbacks.push(rollback)
  }

  commit(): void {
    for (const handle of this.handles) pending.delete(handle)
    this.handles.length = 0
    this.rollbacks.length = 0
  }

  rollback(): void {
    for (const handle of this.handles) pending.delete(handle)
    this.handles.length = 0
    for (let i = this.rollbacks.length - 1; i >= 0; i--) {
      this.rollbacks[i]!()
    }
    this.rollbacks.length = 0
  }
}

export function withCapabilityAdoptionTransaction<T>(
  convert: (transaction: CapabilityTransaction) => T,
): T {
  const transaction = new CapabilityTransaction()
  try {
    const result = convert(transaction)
    transaction.commit()
    return result
  } catch (error) {
    transaction.rollback()
    throw error
  }
}

export const withCapabilityTransaction = <A, E, R>(
  convert: (transaction: CapabilityTransaction) => Effect.Effect<A, E, R>,
): Effect.Effect<A, E, R> =>
  Effect.suspend(() => {
    const transaction = new CapabilityTransaction()
    return Effect.onExit(
      Effect.suspend(() => convert(transaction)),
      (exit) =>
        Effect.sync(() => {
          if (Exit.isSuccess(exit)) transaction.commit()
          else transaction.rollback()
        }),
    )
  })
