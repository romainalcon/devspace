// Labels shown by the interface.

import type { IntegrationStatus, ProcessState, SiteTarget } from "@/lib/protocol";

/** Visual weight of a state. */
export type Tone = "success" | "warning" | "danger" | "neutral";

/** `1h 02m 03s`, as `devspace daemon status`. */
export function formatUptime(secs: number): string {
  const days = Math.floor(secs / 86_400);
  const hours = Math.floor((secs % 86_400) / 3_600);
  const minutes = Math.floor((secs % 3_600) / 60);
  const seconds = secs % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  if (days > 0) return `${days}d ${pad(hours)}h ${pad(minutes)}m ${pad(seconds)}s`;
  if (hours > 0) return `${hours}h ${pad(minutes)}m ${pad(seconds)}s`;
  if (minutes > 0) return `${minutes}m ${pad(seconds)}s`;
  return `${seconds}s`;
}

export function processLabel(state: ProcessState): { label: string; tone: Tone } {
  switch (state.state) {
    case "running":
      return { label: "Running", tone: "success" };
    case "starting":
      return { label: "Starting", tone: "warning" };
    case "failed":
      return { label: "Failed", tone: "danger" };
    case "stopped":
      return { label: "Stopped", tone: "neutral" };
  }
}

export function integrationLabel(status: IntegrationStatus): { label: string; tone: Tone } {
  switch (status) {
    case "installed":
      return { label: "Installed", tone: "success" };
    case "missing":
      return { label: "Not installed", tone: "warning" };
    case "outdated":
      return { label: "Outdated", tone: "warning" };
    case "conflict":
      return { label: "Conflict", tone: "danger" };
    case "unsupported":
      return { label: "Not supported here", tone: "neutral" };
    case "unknown":
      return { label: "Unknown", tone: "neutral" };
  }
}

export function siteTargetLabel(target: SiteTarget): string {
  switch (target.type) {
    case "static":
      return "Static files";
    case "php":
      return `PHP ${target.version}`;
    case "proxy":
      return `Proxy to ${target.upstream}`;
    case "unavailable":
      return "Not served";
  }
}

/** Display names of the packages; unknown names are shown as they are. */
const PACKAGE_NAMES: Record<string, string> = {
  php: "PHP",
  node: "Node.js",
  postgresql: "PostgreSQL",
  mysql: "MySQL",
  valkey: "Valkey",
};

export function packageName(name: string): string {
  return PACKAGE_NAMES[name] ?? name;
}

/**
 * Packages providing the service servers, shown on the Services page
 * rather than with the language runtimes (`devspace service install`).
 */
export const SERVICE_PACKAGES: readonly string[] = ["postgresql", "mysql", "valkey"];

export function formatDate(epochSecs: number): string {
  return new Date(epochSecs * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
