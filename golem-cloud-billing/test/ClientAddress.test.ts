import { Option } from "effect"
import { HttpServerRequest } from "effect/http"
import { describe, expect, it } from "vitest"
import { clientAddress } from "../src/Http.ts"

const requestWith = (headers: Record<string, string>, remoteAddress?: string) =>
  HttpServerRequest.fromWeb(new Request("http://localhost:3000/", { headers })).modify({
    remoteAddress: Option.fromUndefinedOr(remoteAddress)
  })

describe("clientAddress", () => {
  it("takes the last X-Forwarded-For address, which the gateway appends", () => {
    expect(clientAddress(requestWith({ "x-forwarded-for": "203.0.113.9, 198.51.100.7" }, "10.0.0.5"))).toBe(
      "198.51.100.7"
    )
  })

  it("takes the one X-Forwarded-For address", () => {
    expect(clientAddress(requestWith({ "x-forwarded-for": "198.51.100.7" }, "10.0.0.5"))).toBe("198.51.100.7")
  })

  it("takes the address of the connection without X-Forwarded-For", () => {
    expect(clientAddress(requestWith({}, "192.0.2.44"))).toBe("192.0.2.44")
    expect(clientAddress(requestWith({ "x-forwarded-for": "" }, "192.0.2.44"))).toBe("192.0.2.44")
  })

  it("has no address without X-Forwarded-For and without the address of the connection", () => {
    expect(clientAddress(requestWith({}))).toBeUndefined()
  })
})
