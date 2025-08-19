/// <reference types="vitest" />
/* eslint-disable @typescript-eslint/no-unused-vars */
import type { vi } from "vitest";

declare global {
  const vi: typeof import("vitest").vi;
}

export {};
