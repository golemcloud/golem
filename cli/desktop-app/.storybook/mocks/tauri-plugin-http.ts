export async function fetch(
  _url: string,
  _options?: RequestInit,
): Promise<Response> {
  return new Response(JSON.stringify({}), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}
