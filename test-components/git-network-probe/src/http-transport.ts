import { Buffer } from "node:buffer";
import type { HttpClient } from "isomorphic-git";

export type Credentials = {
  origin: string;
  username?: string;
  password?: string;
  bearer?: string;
  headers?: Record<string, string>;
};

export function httpUrl(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("remote must be an HTTP or HTTPS URL");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("SSH, git://, and filesystem transports are not provided");
  }
  if (url.username || url.password) {
    throw new Error(
      "URL credentials are not stored; pass an opaque authentication secret",
    );
  }
  if (url.hash) throw new Error("remote URLs must not contain fragments");
  return url;
}

export function redact(value: string, credentials?: Credentials): string {
  let result = value.replace(/(https?:\/\/)[^\s/@]+@/gi, "$1[redacted]@");
  for (const secret of [
    credentials?.password,
    credentials?.bearer,
    credentials?.password !== undefined
      ? Buffer.from(
          `${credentials.username ?? ""}:${credentials.password}`,
        ).toString("base64")
      : undefined,
    ...Object.values(credentials?.headers ?? {}),
  ]) {
    if (secret) result = result.split(secret).join("[redacted]");
  }
  return result;
}

function authHeaders(credentials: Credentials): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(credentials.headers ?? {})) {
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/i.test(name) || /[\r\n\0]/.test(value)) {
      throw new Error("invalid authentication header");
    }
    if (
      ["host", "content-length", "content-type", "transfer-encoding"].includes(
        name.toLowerCase(),
      )
    ) {
      throw new Error(
        "authentication headers must not override transport headers",
      );
    }
    headers[name.toLowerCase()] = value;
  }
  if (credentials.bearer !== undefined) {
    if (
      credentials.password !== undefined ||
      headers.authorization !== undefined
    ) {
      throw new Error("choose one authentication method");
    }
    if (/[\r\n\0]/.test(credentials.bearer))
      throw new Error("invalid bearer credential");
    headers.authorization = `Bearer ${credentials.bearer}`;
  } else if (credentials.password !== undefined) {
    if (headers.authorization !== undefined)
      throw new Error("choose one authentication method");
    if (credentials.username?.includes(":"))
      throw new Error("Basic authentication username must not contain ':'");
    headers.authorization = `Basic ${Buffer.from(`${credentials.username ?? ""}:${credentials.password}`).toString("base64")}`;
  }
  return headers;
}

/** The injected client exposes raw WASI HTTP responses, including redirects. */
export function httpTransport(
  credentials: Credentials | undefined,
  send: typeof fetch,
  followRedirects: "initial" | "true" | "false" = "initial",
): HttpClient & {
  run<T>(operation: (http: HttpClient) => Promise<T>): Promise<T>;
} {
  const credentialOrigin = credentials
    ? httpUrl(credentials.origin).origin
    : undefined;
  const secretHeaders = credentials ? authHeaders(credentials) : {};
  const canonicalBases = new Map<string, string>();
  const readers = new Set<ReadableStreamDefaultReader<Uint8Array>>();
  let failure: Error | undefined;
  function transportError(error: unknown): Error {
    return new Error(
      redact(
        error instanceof Error ? error.message : String(error),
        credentials,
      ),
    );
  }
  async function close(reader: ReadableStreamDefaultReader<Uint8Array>) {
    if (!readers.delete(reader)) return;
    try {
      await reader.cancel();
    } catch (error) {
      failure = transportError(error);
      throw failure;
    } finally {
      reader.releaseLock();
    }
  }
  const transport = {
    async run<T>(operation: (http: HttpClient) => Promise<T>): Promise<T> {
      try {
        const result = await operation(transport);
        if (failure) throw failure;
        return result;
      } finally {
        await Promise.all([...readers].map(close));
      }
    },
    async request(request: Parameters<HttpClient["request"]>[0]) {
      let original = httpUrl(request.url);
      for (const [from, to] of canonicalBases) {
        if (original.href.startsWith(`${from}/`)) {
          original = httpUrl(to + original.href.slice(from.length));
          break;
        }
      }
      let url = original;
      const method = request.method ?? "GET";
      const chunks: Uint8Array[] = [];
      if (request.body)
        for await (const chunk of request.body) chunks.push(chunk);
      const body = request.body ? Buffer.concat(chunks) : undefined;
      const headers = { ...request.headers };
      let credentialAllowed = original.origin === credentialOrigin;
      try {
        for (let redirects = 0; ; redirects += 1) {
          const response = await send(url.href, {
            method,
            headers: {
              ...headers,
              ...(credentialAllowed ? secretHeaders : {}),
            },
            body,
            redirect: "manual",
            signal: request.signal,
          });
          const reader = response.body?.getReader();
          if (reader) readers.add(reader);
          if ([301, 302, 303, 307, 308].includes(response.status)) {
            const location = response.headers.get("location");
            if (reader) await close(reader);
            if (
              followRedirects === "false" ||
              (followRedirects === "initial" && method !== "GET")
            ) {
              throw new Error(
                "remote redirect is disabled by http.followRedirects",
              );
            }
            if (!location) throw new Error("remote redirect has no Location");
            if (redirects >= 20) throw new Error("too many remote redirects");
            const next = httpUrl(new URL(location, url).href);
            if (next.origin !== url.origin) {
              credentialAllowed = false;
              for (const name of Object.keys(headers)) {
                if (
                  ["authorization", "proxy-authorization", "cookie"].includes(
                    name.toLowerCase(),
                  )
                )
                  delete headers[name];
              }
            }
            url = next;
            continue;
          }
          if (response.status === 401 || response.status === 403) {
            if (reader) await close(reader);
            throw new Error(
              `remote authentication failed (HTTP ${response.status}); noninteractive credentials are missing, expired, or denied`,
            );
          }
          if (
            method === "GET" &&
            original.pathname.endsWith("/info/refs") &&
            url.href !== original.href
          ) {
            if (
              !url.pathname.endsWith("/info/refs") ||
              url.search !== original.search
            ) {
              if (reader) await close(reader);
              throw new Error(
                "remote discovery redirect does not identify a Git repository base",
              );
            }
            canonicalBases.set(
              original.href.slice(0, original.href.indexOf("/info/refs")),
              url.href.slice(0, url.href.indexOf("/info/refs")),
            );
          }
          return {
            url: url.href,
            method,
            statusCode: response.status,
            statusMessage: response.statusText,
            headers: Object.fromEntries(response.headers.entries()),
            body: (async function* () {
              if (!reader) return;
              try {
                for (;;) {
                  const { done, value } = await reader.read();
                  if (done) return;
                  yield value;
                }
              } catch (error) {
                failure = transportError(error);
                throw failure;
              } finally {
                await close(reader);
              }
            })(),
          };
        }
      } catch (error) {
        throw new Error(
          redact(
            error instanceof Error ? error.message : String(error),
            credentials,
          ),
        );
      }
    },
  };
  return transport;
}
