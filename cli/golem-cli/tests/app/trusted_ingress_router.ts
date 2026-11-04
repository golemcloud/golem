import { defineHttpRouter } from "@golemcloud/golem-ts-sdk";

export const OriginProbe = defineHttpRouter("OriginProbe")
  .mount("/probe")
  .implement((request) => new Response(request.url));
