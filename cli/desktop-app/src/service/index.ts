import { Service } from "@/service/client.ts";
// @ts-nocheck
// import { fetchCurrentIP } from "@/lib/tauri&web.ts";

export let API: Service = new Service();

(async () => {
  API = new Service();
})();
