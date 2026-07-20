# TUI Design Previews

Static previews for comparing Golem TUI visual directions before changing the
Ratatui renderer.

Open `index.html` directly in a browser. The preview is dependency-free and is
not a source of truth for implementation. Accepted decisions should be copied
back into `.tui-plan/ui-system.md`, then recreated in Ratatui and covered by
render tests.

Current preview set:

- A: current rails, closer to the existing amber/dark TUI.
- B: quiet status bar, lower accent weight and softer split language.
- C: dense handles, stronger split affordances and compact operational panels.
- D: ops contrast, clearer Ops/server surfaces and stronger state colors.
