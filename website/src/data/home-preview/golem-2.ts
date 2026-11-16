import approvedCopy from "./golem-2-copy.md?raw";
import approvedDetails from "./golem-2-details.md?raw";

// The build uses the approved copy snapshot without depending on local research files.
function sections(markdown: string): Record<string, string>[] {
  return markdown
    .replace(/<!--[\s\S]*?-->/g, "")
    .split(/^## \d+\. .*$/m)
    .slice(1)
    .map((section) =>
      Object.fromEntries(
        section
          .trim()
          .split(/\n(?=\*\*[^\n]+\*\*\n)/)
          .map((block) => {
            const match = block.match(/^\*\*(.+)\*\*\s*\n([\s\S]*)$/);
            if (!match) throw new Error(`Unrecognized homepage copy: ${block.slice(0, 80)}`);
            return [match[1], match[2].trim()];
          })
      )
    );
}

export const copy = sections(approvedCopy);
export const details = sections(approvedDetails);
export const text = (section: number, key: string) => {
  const value = copy[section - 1]?.[key];
  if (!value) throw new Error(`Missing approved homepage copy: ${section}/${key}`);
  return value;
};
export const paragraphs = (value: string) => value.split(/\n\n+/);
export const lines = (value: string) => value.split("\n").map((line) => line.replace(/^- /, ""));
export const code = text(7, "Example — inside an actor method").replace(
  /^```typescript\n|\n```$/g,
  ""
);
export const instruction = text(9, "Copyable instruction").replace(/^> /, "");
