const groups = [
  [".primitive-copy", ":scope > div > p"],
  [".guarantees", ".guarantee-description"],
  [".use-cases", ":scope > .use-case > p"],
  [".capability-list", ".capability-item > span:nth-child(2) > span"],
  [".agent-features", ":scope > article > p"],
  [".language-copy", ":scope > article > p"],
  [".deployment-options", ":scope > article > p"],
  [".example-outcomes", ":scope > article > p"],
] as const;

// A small shared measure adjustment keeps peer descriptions on the same number
// of real text lines as fonts and available widths change. No text is hidden.
export function balanceDescriptions() {
  const roots = groups.flatMap(([selector, items]) => {
    const root = document.querySelector<HTMLElement>(selector);
    return root ? [{ root, items }] : [];
  });
  const widths = new WeakMap<Element, number>();
  let pending = 0;

  const balance = () => {
    for (const { root, items } of roots) {
      root.style.removeProperty("--parallel-copy-width");
      const paragraphs = [...root.querySelectorAll<HTMLElement>(items)];
      const available = Math.floor(
        Math.min(...paragraphs.map((p) => p.getBoundingClientRect().width))
      );
      const minimum = Math.ceil(available * 0.8);
      for (let width = available; width >= minimum; width--) {
        root.style.setProperty("--parallel-copy-width", `${width}px`);
        const lines = paragraphs.map((p) =>
          Math.round(p.getBoundingClientRect().height / parseFloat(getComputedStyle(p).lineHeight))
        );
        if (Math.min(...lines) === Math.max(...lines)) break;
      }
      widths.set(root, root.getBoundingClientRect().width);
    }
    document.querySelector<HTMLElement>(".g2-page")!.dataset.copyBalanced = "true";
  };

  const schedule = () => {
    cancelAnimationFrame(pending);
    pending = requestAnimationFrame(balance);
  };
  const observer = new ResizeObserver((entries) => {
    if (entries.some(({ target, contentRect }) => widths.get(target) !== contentRect.width))
      schedule();
  });
  roots.forEach(({ root }) => observer.observe(root));
  void document.fonts.ready.then(schedule);
  window.addEventListener("resize", schedule);
  schedule();
}
