// What the interface last received from the daemon, refreshed periodically
// while the window is visible and right after every action.

import { reactive } from "vue";

import { daemon, errorMessage, request } from "@/lib/daemon";
import type {
  DaemonView,
  ProjectInfo,
  RuntimeList,
  ServiceList,
  WebStatus,
} from "@/lib/protocol";

/** Daemon state, projects and services. */
const FAST_PERIOD_MS = 2_000;
/** Web server and package index: their status runs system checks. */
const SLOW_PERIOD_MS = 10_000;

interface Snapshot {
  daemon: DaemonView | null;
  projects: ProjectInfo[] | null;
  services: ServiceList | null;
  web: WebStatus | null;
  runtimes: RuntimeList | null;
  /** Failure of the last refresh, per part. */
  errors: Partial<Record<"projects" | "services" | "web" | "runtimes", string>>;
}

export const store = reactive<Snapshot>({
  daemon: null,
  projects: null,
  services: null,
  web: null,
  runtimes: null,
  errors: {},
});

export function isRunning(): boolean {
  return store.daemon?.state === "running";
}

async function load<K extends keyof Snapshot["errors"]>(
  key: K,
  fetch: () => Promise<Snapshot[K]>,
): Promise<void> {
  try {
    store[key] = await fetch();
    delete store.errors[key];
  } catch (error) {
    store.errors[key] = errorMessage(error);
  }
}

async function refreshFast(): Promise<void> {
  store.daemon = await daemon.status().catch(
    (error): DaemonView => ({ state: "unknown", message: errorMessage(error) }),
  );
  if (!isRunning()) return;
  await Promise.all([
    load("projects", async () => (await request({ method: "project_list" }, "projects")).projects),
    load("services", () => request({ method: "service_list" }, "services")),
  ]);
}

async function refreshSlow(): Promise<void> {
  if (!isRunning()) return;
  await Promise.all([
    load("web", () => request({ method: "web_status" }, "web_status")),
    load("runtimes", () => request({ method: "runtime_list" }, "runtimes")),
  ]);
}

/** Refreshes everything now. */
export async function refresh(): Promise<void> {
  await refreshFast();
  await refreshSlow();
}

/** Keeps the snapshot up to date until the returned function is called. */
export function startPolling(): () => void {
  let fastTimer: ReturnType<typeof setTimeout> | undefined;
  let slowTimer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;

  // A cycle starts after the previous one ended, so slow answers never pile up.
  const cycle = (run: () => Promise<void>, period: number, set: (t: ReturnType<typeof setTimeout>) => void) => {
    const tick = async () => {
      if (!stopped && document.visibilityState === "visible") await run();
      if (!stopped) set(setTimeout(tick, period));
    };
    return tick;
  };
  const fast = cycle(refreshFast, FAST_PERIOD_MS, (t) => (fastTimer = t));
  const slow = cycle(refreshSlow, SLOW_PERIOD_MS, (t) => (slowTimer = t));

  const onVisible = () => {
    if (document.visibilityState === "visible") void refresh();
  };
  document.addEventListener("visibilitychange", onVisible);
  void fast().then(() => slow());

  return () => {
    stopped = true;
    clearTimeout(fastTimer);
    clearTimeout(slowTimer);
    document.removeEventListener("visibilitychange", onVisible);
  };
}
