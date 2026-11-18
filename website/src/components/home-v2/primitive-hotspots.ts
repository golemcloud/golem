type Topic = "actor" | "invocation" | "function";

export function setupPrimitiveHotspots() {
  const root = document.querySelector<HTMLElement>(".primitive-comparison")!;
  const buttons = [...root.querySelectorAll<HTMLButtonElement>(".diagram-hotspot")];
  const cards = [...document.querySelectorAll<HTMLElement>("[data-primitive-card]")];
  let active: Topic | undefined;
  let pinned = false;
  let dismissTimer = 0;
  let ignoreFocus = false;

  const buttonFor = (topic: Topic) => buttons.find((b) => b.dataset.topic === topic)!;
  const cardFor = (topic: Topic) => cards.find((c) => c.dataset.primitiveCard === topic)!;

  function placeCard() {
    if (!active) return;
    const card = cardFor(active);
    const button = buttonFor(active);
    const anchor = button.querySelector<HTMLElement>(".hotspot-icon")!.getBoundingClientRect();
    if (anchor.bottom < 0 || anchor.top > innerHeight) {
      close(false);
      return;
    }
    if (matchMedia("(max-width: 600px)").matches) return;
    const bounds = card.getBoundingClientRect();
    const column = button.closest("article")!.getBoundingClientRect();
    const figure = button.closest("figure")!.getBoundingClientRect();
    const belowFigure = figure.bottom + 12;
    const useBelowFigure = belowFigure + bounds.height <= innerHeight - 20;
    const x = anchor.x + anchor.width / 2;
    const y = anchor.y + anchor.height / 2;
    const left = Math.max(
      20,
      Math.min(
        useBelowFigure ? column.right - bounds.width : x - bounds.width / 2,
        column.right - bounds.width,
        innerWidth - bounds.width - 20
      )
    );
    const below = anchor.bottom + 12;
    const above = anchor.top - bounds.height - 12;
    const preferBelow = active === "actor";
    const top = Math.max(
      20,
      Math.min(
        useBelowFigure
          ? belowFigure
          : preferBelow && below + bounds.height <= innerHeight - 20
            ? below
            : above >= 20
              ? above
              : below,
        innerHeight - bounds.height - 20
      )
    );
    card.style.setProperty("--card-left", `${left}px`);
    card.style.setProperty("--card-top", `${top}px`);
    const endX = Math.max(18, Math.min(x - left, bounds.width - 18));
    const endY = top > y ? 0 : bounds.height;
    const path = useBelowFigure
      ? `M ${x - left} ${y - top} H ${column.right + 14 - left} V 20 H ${bounds.width}`
      : `M ${x - left} ${y - top} L ${endX} ${endY}`;
    card.querySelector("path")!.setAttribute("d", path);
  }

  function close(restoreFocus: boolean) {
    clearTimeout(dismissTimer);
    if (!active) return;
    const previous = buttonFor(active);
    cardFor(active).hidePopover();
    previous.setAttribute("aria-expanded", "false");
    delete root.dataset.activeTopic;
    active = undefined;
    pinned = false;
    if (restoreFocus) {
      ignoreFocus = true;
      previous.focus({ preventScroll: true });
      queueMicrotask(() => {
        ignoreFocus = false;
      });
    }
  }

  function show(topic: Topic) {
    clearTimeout(dismissTimer);
    if (active === topic) return;
    if (pinned) return;
    close(false);
    active = topic;
    root.dataset.activeTopic = topic;
    buttonFor(topic).setAttribute("aria-expanded", "true");
    const card = cardFor(topic);
    card.style.visibility = "hidden";
    card.showPopover();
    placeCard();
    card.style.visibility = "";
  }

  function scheduleDismiss() {
    clearTimeout(dismissTimer);
    dismissTimer = window.setTimeout(() => {
      if (!active || pinned) return;
      const card = cardFor(active);
      const button = buttonFor(active);
      if (
        card.matches(":hover") ||
        button.matches(":hover") ||
        card.contains(document.activeElement) ||
        button === document.activeElement
      )
        return;
      close(false);
    }, 450);
  }

  for (const button of buttons) {
    const topic = button.dataset.topic as Topic;
    button.addEventListener("pointerenter", (event) => {
      if (event.pointerType === "mouse") show(topic);
    });
    button.addEventListener("pointerleave", scheduleDismiss);
    button.addEventListener("focus", () => {
      if (!ignoreFocus) show(topic);
    });
    button.addEventListener("blur", scheduleDismiss);
    button.addEventListener("click", (event) => {
      if (active === topic && pinned) {
        close(false);
        return;
      }
      pinned = false;
      show(topic);
      pinned = true;
      if (event.detail === 0) cardFor(topic).focus({ preventScroll: true });
    });
  }
  for (const card of cards) {
    card.addEventListener("pointerenter", () => clearTimeout(dismissTimer));
    card.addEventListener("pointerleave", scheduleDismiss);
    card.addEventListener("focusout", scheduleDismiss);
    card.querySelector(".primitive-close")!.addEventListener("click", () => close(true));
  }
  document.addEventListener("pointerdown", (event) => {
    if (!active || !(event.target instanceof Node)) return;
    if (
      !cardFor(active).contains(event.target) &&
      !buttons.some((b) => b.contains(event.target as Node))
    )
      close(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && active) {
      event.preventDefault();
      close(true);
    }
  });
  const refresh = () => {
    placeCard();
  };
  const introduction = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting || entry.intersectionRatio < 0.8) continue;
        entry.target.classList.add("is-introduced");
        introduction.unobserve(entry.target);
      }
    },
    { threshold: 0.8 }
  );
  buttons.forEach((button) => introduction.observe(button));
  new ResizeObserver(refresh).observe(root);
  window.addEventListener("resize", refresh);
  window.addEventListener("scroll", placeCard, { capture: true, passive: true });
  void document.fonts.ready.then(refresh);
  refresh();
}
