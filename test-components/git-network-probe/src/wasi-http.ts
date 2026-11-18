import * as http from "node:http";
import * as https from "node:https";

/** The pinned P3 runtime's Node HTTP clients send exclusively through WASI HTTP. */
export const wasiHttp: typeof fetch = async (input, options = {}) => {
  const url = new URL(String(input));
  const client = url.protocol === "https:" ? https : http;
  if (options.body !== undefined && options.body !== null) {
    if (!ArrayBuffer.isView(options.body))
      throw new Error("Git HTTP body must be bytes");
  }
  return new Promise<Response>((resolve, reject) => {
    const request = client.request(
      url,
      {
        method: options.method ?? "GET",
        headers: Object.fromEntries(new Headers(options.headers).entries()),
        signal: options.signal ?? undefined,
        agent: false,
      },
      (response) => {
        const closed = new Promise<void>((resolve) => {
          response.once("close", resolve);
        });
        response.on("error", reject);
        try {
          const headers = new Headers();
          for (let index = 0; index < response.rawHeaders.length; index += 2) {
            headers.append(
              response.rawHeaders[index]!,
              response.rawHeaders[index + 1]!,
            );
          }
          const iterator = response[Symbol.asyncIterator]();
          resolve(
            new Response(
              new ReadableStream<Uint8Array>(
                {
                  async pull(controller) {
                    try {
                      const { done, value } = await iterator.next();
                      if (done) {
                        await closed;
                        controller.close();
                      } else controller.enqueue(value);
                    } catch (error) {
                      controller.error(error);
                    }
                  },
                  async cancel() {
                    response.destroy();
                    await closed;
                  },
                },
                { highWaterMark: 0 },
              ),
              {
                status: response.statusCode,
                statusText: response.statusMessage,
                headers,
              },
            ),
          );
        } catch (error) {
          response.destroy();
          void closed.then(() => reject(error));
        }
      },
    );
    request.on("error", reject);
    request.end(options.body ?? undefined);
  });
};
