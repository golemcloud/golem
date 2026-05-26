// Build-time fetch for GitHub star count. Memoized at module scope so multiple
// components rendering on the same dev server / build invocation share one call.

let cached: string | null | undefined;

export async function getGitHubStars(repo = "golemcloud/golem"): Promise<string | null> {
  if (cached !== undefined) return cached;
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}`, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!res.ok) {
      cached = null;
      return null;
    }
    const data = (await res.json()) as { stargazers_count?: number };
    const count = data.stargazers_count;
    if (typeof count !== "number") {
      cached = null;
      return null;
    }
    cached = count >= 1000 ? `${(count / 1000).toFixed(1)}k` : String(count);
    return cached;
  } catch {
    cached = null;
    return null;
  }
}
