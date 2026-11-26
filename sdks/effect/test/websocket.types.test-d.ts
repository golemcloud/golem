import { Context, Effect, Scope } from "effect"
import { Socket } from "effect/socket"
import { expectTypeOf } from "vitest"
import type { GolemSocket } from "../src/Websocket.js"

declare const socket: GolemSocket

const read = socket.runString(() => {})
expectTypeOf(read).toEqualTypeOf<Effect.Effect<void, Socket.SocketError, Scope.Scope>>()
expectTypeOf(Effect.scoped(read)).toEqualTypeOf<Effect.Effect<void, Socket.SocketError>>()

class HandlerService extends Context.Service<HandlerService, { readonly value: number }>()(
  "HandlerService",
) {}

declare const handler: (line: string) => Effect.Effect<number, "HandlerError", HandlerService>
const effectfulRead = socket.runString(handler)
expectTypeOf(effectfulRead).toEqualTypeOf<
  Effect.Effect<void, Socket.SocketError | "HandlerError", Scope.Scope | HandlerService>
>()
