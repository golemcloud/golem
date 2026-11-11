import { describe, expect, it } from "vitest"
import { breakAtHyphens, cliBootstrapCommand, html, SafeHtml } from "../src/Page.ts"

describe("html", () => {
  it("escapes interpolated strings", () => {
    expect(html`<p>${`<script>alert("x")</script>`}</p>`.value).toBe(
      "<p>&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;</p>"
    )
  })

  it("inserts SafeHtml as it is", () => {
    expect(html`<p>${new SafeHtml("<b>bold</b>")}</p>`.value).toBe("<p><b>bold</b></p>")
  })
})

describe("breakAtHyphens", () => {
  it("lets a UUID wrap only after its hyphens", () => {
    expect(breakAtHyphens("01a1c567-0cc6-7d91-943e-998dafbebc78").value).toBe(
      "01a1c567-<wbr>0cc6-<wbr>7d91-<wbr>943e-<wbr>998dafbebc78"
    )
  })

  it("escapes the value", () => {
    expect(breakAtHyphens("<a>-b").value).toBe("&lt;a&gt;-<wbr>b")
  })
})

describe("cliBootstrapCommand", () => {
  it("uses the built-in cloud profile for the default cloud URL", () => {
    expect(cliBootstrapCommand(new URL("https://release.api.golem.cloud"))).toBe("golem --cloud account get")
  })

  it("creates a profile for another Golem API URL", () => {
    expect(cliBootstrapCommand(new URL("https://release.dev-api.golem.cloud/"))).toBe(
      "golem profile new golem-cloud --url https://release.dev-api.golem.cloud --set-active\ngolem account get"
    )
  })
})
