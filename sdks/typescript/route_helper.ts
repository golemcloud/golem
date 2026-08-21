export function route(baseUrl: string, pattern: string, params: Record<string, string>): string {
  let path = pattern;
  for (const [k, v] of Object.entries(params)) {
    path = path.replace(`{${k}}`, encodeURIComponent(v));
  }
  return new URL(path, baseUrl).toString();
}
