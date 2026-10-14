import { createServer } from 'node:http';

const state = globalThis as any;
state.__golemAutoEvaluated = true;
const server = createServer((_req, res) => res.end(`${server.listening}:${server.address()}`));
state.__golemAutoServer = server;
state.__golemAutoCreateServer = createServer;
server.on('listening', () => {
  state.__golemAutoEvents = (state.__golemAutoEvents ?? 0) + 1;
});
if (state.__golemAutoWait) await state.__golemAutoWait;
server.listen(3000, function () {
  state.__golemAutoCallback = this === server && this.listening && this.address() === null;
});
if (state.__golemAutoFail) {
  if (state.__golemAutoFail === 'async') await Promise.reject(state.__golemAutoError);
  else throw state.__golemAutoError;
}
