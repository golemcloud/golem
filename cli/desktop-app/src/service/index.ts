import { Service } from "@/service/client.ts";
// @ts-nocheck
import { fetchCurrentIP } from "@/lib/tauri&web.ts";

export let API: Service = new Service("http://localhost:9881");

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