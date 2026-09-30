declare module 'user' {
  const application: Promise<{
    guest: typeof import('agent-guest').golemAgent200Guest;
    tool: typeof import('agent-guest').golemTool010Guest;
    toolMiddlewareGuest: typeof import('agent-guest').toolMiddlewareGuest;
    saveSnapshot: typeof import('agent-guest').saveSnapshot;
    loadSnapshot: typeof import('agent-guest').loadSnapshot;
  }>;
  export default application;
}
