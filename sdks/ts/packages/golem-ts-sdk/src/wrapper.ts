// Resolve registration during module initialization, not during a synchronous
// WIT export: discovery and get-definition must return values, never promises.
import application from 'user';

export const { guest, tool, toolMiddlewareGuest, saveSnapshot, loadSnapshot } = await application;
export const golemAgent200Guest = guest;
export const golemTool010Guest = tool;
export const golemTool010ToolMiddlewareGuest = toolMiddlewareGuest;
