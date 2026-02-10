import { BaseDirectory } from "@tauri-apps/api/path";
// import { invoke } from "@tauri-apps/api/core";
import { listen, TauriEvent as TauriEventEnum } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import { writeFile } from "@tauri-apps/plugin-fs";

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

export async function saveFile(fileName: string, data: Uint8Array) {
  if (isTauri) {
    // Use Tauri to save the file in the Downloads directory
    await writeFile(fileName, data, { baseDir: BaseDirectory.Download });
  } else {
    // Use Blob and createObjectURL for web downloads
    const blob = new Blob([data]);
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = fileName;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }
}

export async function fetchData(
  url: string,
  options?: RequestInit,
): Promise<Response> {
  if (isTauri) {
    return tauriFetch(url, options); // Use Tauri HTTP plugin
  } else {
    return fetch(url, options); // Use standard browser fetch
  }
}
