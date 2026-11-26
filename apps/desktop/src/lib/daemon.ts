// Calls to the desktop shell commands (`src-tauri/src/lib.rs`). Every
// operation is performed by the daemon; the interface holds no state of its
// own beyond what it last received.

import { invoke } from "@tauri-apps/api/core";

import type {
  DaemonView,
  Request,
  Response,
  ResponseKind,
} from "@/lib/protocol";

/** A refusal or failure, with the message to show. */
export class DaemonError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DaemonError";
  }
}

/** Message of anything thrown by a command. */
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return String(error);
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw new DaemonError(errorMessage(error));
  }
}

/** Sends `request` and returns the response of the `expected` kind. */
export async function request<K extends ResponseKind>(
  request: Request,
  expected: K,
): Promise<Extract<Response, { kind: K }>> {
  const response = await call<Response>("daemon_request", { request });
  if (response.kind === "error") throw new DaemonError(response.message);
  if (response.kind !== expected) {
    throw new DaemonError(
      `unexpected answer from the daemon: expected ${expected}, got ${response.kind}`,
    );
  }
  return response as Extract<Response, { kind: K }>;
}

export const daemon = {
  status: () => call<DaemonView>("daemon_status"),
  start: () => call<DaemonView>("daemon_start"),
  stop: () => call<DaemonView>("daemon_stop"),
  openSite: (project: string) => call<void>("open_site", { project }),
  revealProject: (project: string) => call<void>("reveal_project", { project }),
};
