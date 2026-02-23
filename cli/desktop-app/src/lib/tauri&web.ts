// import { invoke } from "@tauri-apps/api/core";
import { listen, TauriEvent as TauriEventEnum } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";

const isTauri =
  typeof window !== "undefined" &&
  (window as { __TAURI__?: unknown }).__TAURI__;

export type Theme = "dark" | "light" | "system";

export function listenThemeChange(
  cb: (event: Event<Exclude<Theme, "system">>) => void,
) {
  if (isTauri) {
    // To cancel Tauri listener we have to wait for promise resolution,
    // since this function is only expected to be used in useEffect we
    // cannot expose async nature of Tauri outside of it.
    const unlistenPromise = listen(TauriEventEnum.WINDOW_THEME_CHANGED, cb);
    const unlistenCallback = () => unlistenPromise.then(unlisten => unlisten());

    return unlistenCallback;
  } else {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = (event: MediaQueryListEvent) =>
      event.matches ? "dark" : "light";
    media.addEventListener("change", handler);
    return () => media.removeEventListener("change", handler);
  }
}
