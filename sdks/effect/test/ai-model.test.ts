import { describe, expect, it } from "vitest"
import { command, commandDescription, resolveCommandSelections } from "../src/Ai.js"

describe("AI tool model", () => {
  it("names explicitly selected root and nested commands deterministically", () => {
    expect(
      resolveCommandSelections("files", [command(), command(["archive", "create"])]).map(
        ({ name }) => name,
      ),
    ).toEqual(["files", "files__archive__create"])
  })

  it("applies overrides and rejects generated or overridden collisions", () => {
    expect(resolveCommandSelections("files", [command(["read"], { name: "fetch" })])[0].name).toBe(
      "fetch",
    )
    expect(() =>
      resolveCommandSelections("files", [command(), command(["read"], { name: "files" })]),
    ).toThrow(/duplicate AI tool name 'files'/)
    expect(() => resolveCommandSelections("files", [command(["read"]), command(["read"])])).toThrow(
      /duplicate AI tool name 'files__read'/,
    )
  })

  it("validates names, paths, and finite capture limits at construction", () => {
    expect(() => command([""])).toThrow(/path segments/)
    expect(() => command([], { name: "" })).toThrow(/names/)
    expect(() => command([], { name: "__proto__" })).toThrow(/not supported/)
    expect(() => resolveCommandSelections("__proto__", [command()])).toThrow(/not supported/)
    for (const maxStdoutBytes of [-1, 0.5, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => command([], { maxStdoutBytes })).toThrow(/non-negative safe integer/)
    }
    expect(command([], { maxStdoutBytes: 0 }).options.maxStdoutBytes).toBe(0)
  })

  it("combines command and stream documentation with MIME hints", () => {
    expect(
      commandDescription(
        { summary: "Convert a document", description: "Preserves page order." },
        {
          required: true,
          mime: ["text/plain", "application/octet-stream"],
          doc: { description: "The source document." },
        },
        { mime: ["application/pdf"], doc: { summary: "Rendered PDF." } },
      ),
    ).toBe(
      [
        "Convert a document",
        "Preserves page order.",
        'Stdin is required; pass _stdin as { data, encoding: "utf8" | "base64" }. Declared MIME hints: text/plain, application/octet-stream.',
        "The source document.",
        "Stdout is returned as bounded captured bytes after the stream is fully drained. Declared MIME hints: application/pdf.",
        "Rendered PDF.",
      ].join("\n\n"),
    )
  })
})
