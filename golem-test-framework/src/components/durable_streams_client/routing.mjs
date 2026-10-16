import { Agent, buildConnector, setGlobalDispatcher } from "undici";

// Preserve the deployment's URL/Host while connecting to the local test server.
const origin = new URL(process.env.GOLEM_TEST_ORIGIN);
const connect = buildConnector({});
setGlobalDispatcher(new Agent({
  connect(options, callback) {
    return connect({
      ...options,
      hostname: origin.hostname,
      host: origin.host,
      port: origin.port,
    }, callback);
  },
}));
