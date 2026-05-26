import rss from "@astrojs/rss";
import { getCollection } from "astro:content";

export async function GET(context) {
  const posts = await getCollection("blog", ({ data }) => !data.draft);
  const sorted = posts.sort((a, b) => {
    const da = a.data.date?.valueOf() ?? -Infinity;
    const db = b.data.date?.valueOf() ?? -Infinity;
    return db - da;
  });
  return rss({
    title: "Golem Blog",
    description: "Engineering, product, and industry articles from the Golem Cloud team.",
    site: context.site,
    items: sorted.map((p) => ({
      title: p.data.title,
      pubDate: p.data.date ?? new Date(2024, 0, 1),
      description: p.data.description ?? "",
      link: `/blog/${p.data.slug ?? p.id}/`,
      author: p.data.author ?? "Golem Cloud Team",
      categories: p.data.tags,
    })),
    customData: "<language>en-us</language>",
  });
}
