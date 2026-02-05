export async function listen(
  event: string,
  handler: (event: unknown) => void
): Promise<() => void> {
  void event;
  void handler;
  return () => {};
}

export async function emit(event: string, payload?: unknown): Promise<void> {
  void event;
  void payload;
}

export const TauriEvent = {
  WINDOW_RESIZED: "tauri://resize",
  WINDOW_MOVED: "tauri://move",
  WINDOW_CLOSE_REQUESTED: "tauri://close-requested",
  WINDOW_DESTROYED: "tauri://destroyed",
  WINDOW_FOCUS: "tauri://focus",
  WINDOW_BLUR: "tauri://blur",
  WINDOW_SCALE_FACTOR_CHANGED: "tauri://scale-change",
  WINDOW_THEME_CHANGED: "tauri://theme-changed",
  WEBVIEW_CREATED: "tauri://webview-created",
  DRAG_ENTER: "tauri://drag-enter",
  DRAG_OVER: "tauri://drag-over",
  DRAG_DROP: "tauri://drag-drop",
  DRAG_LEAVE: "tauri://drag-leave",
} as const;

export type Event<T> = {
  event: string;
  id: number;
  payload: T;
};
