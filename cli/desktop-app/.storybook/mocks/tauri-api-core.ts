export async function invoke(): Promise<string> {
  return "[]";
}

export function transformCallback(
  callback?: (response: unknown) => void,
  once?: boolean
): number {
  void callback;
  void once;
  return 0;
}

export async function addPluginListener(
  plugin: string,
  event: string,
  cb: (payload: unknown) => void
): Promise<{ unregister: () => void }> {
  void plugin;
  void event;
  void cb;
  return { unregister: () => {} };
}

export function removePluginListener(id: number): void {
  void id;
}
