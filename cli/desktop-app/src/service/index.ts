// @ts-nocheck
import { fetchCurrentIP } from "@/lib/tauri&web.ts";
import { Service } from "@/service/client.ts";

export let API: Service;

(async () => {
  API = new Service(await fetchCurrentIP());
})();

export async function updateService(url: string) {
  if (API) {
    await API.updateBackendEndpoint(url);
  }
}

export async function getEndpoint() {
  return await fetchCurrentIP();
}
