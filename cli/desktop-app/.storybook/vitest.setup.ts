import * as a11yAddonAnnotations from "@storybook/addon-a11y/preview";
import { setProjectAnnotations } from "@storybook/react-vite";
import * as projectAnnotations from "./preview";

// Suppress benign unhandled rejections from @monaco-editor/loader's makeCancelable.
// When React unmounts a component using useMonaco(), the loader's cancelable promise
// rejects with a cancellation message that has no .catch() handler upstream.
window.addEventListener("unhandledrejection", event => {
  if (event.reason?.type === "cancelation") {
    event.preventDefault();
  }
});

// This is an important step to apply the right configuration when testing your stories.
// More info at: https://storybook.js.org/docs/api/portable-stories/portable-stories-vitest#setprojectannotations
setProjectAnnotations([a11yAddonAnnotations, projectAnnotations]);
