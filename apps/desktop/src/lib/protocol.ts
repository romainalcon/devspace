// Types of the daemon IPC protocol (version 5), as serialised by
// `crates/ipc/src/protocol.rs`, and of the desktop shell commands
// (`src-tauri/src/daemon.rs`). Keep them in sync with the Rust types: the
// shell checks the protocol version with the daemon, not these types.

export type Request =
  | { method: "ping" }
  | { method: "status" }
  | { method: "project_add"; path: string; name?: string }
  | { method: "project_list" }
  | { method: "project_remove"; name: string }
  | { method: "web_install" }
  | { method: "web_start" }
  | { method: "web_stop" }
  | { method: "web_restart" }
  | { method: "web_status" }
  | { method: "runtime_list" }
  | { method: "runtime_install"; name: string; version?: string }
  | { method: "runtime_remove"; name: string; version: string }
  | { method: "runtime_env"; directory: string }
  | { method: "service_list" }
  | { method: "service_start"; name: string; series?: string }
  | { method: "service_stop"; name: string; series?: string };

export type Response =
  | { kind: "pong" }
  | ({ kind: "status" } & DaemonStatus)
  | { kind: "shutting_down" }
  | ({ kind: "project_added" } & ProjectInfo)
  | { kind: "projects"; projects: ProjectInfo[] }
  | ({ kind: "project_removed" } & ProjectInfo)
  | ({ kind: "web_status" } & WebStatus)
  | ({ kind: "runtimes" } & RuntimeList)
  | ({ kind: "runtime_installed" } & RuntimeInfo)
  | ({ kind: "runtime_removed" } & RuntimeInfo)
  | ({ kind: "runtime_env" } & RuntimeEnv)
  | ({ kind: "services" } & ServiceList)
  | ({ kind: "error" } & ErrorResponse);

export type ResponseKind = Response["kind"];

export interface DaemonStatus {
  version: string;
  pid: number;
  uptime_secs: number;
}

export interface ProjectInfo {
  name: string;
  path: string;
  /** Seconds since the Unix epoch. */
  added_at: number;
}

export type ErrorCode =
  | "invalid_request"
  | "already_exists"
  | "not_found"
  | "unavailable"
  | "failed"
  | "internal";

export interface ErrorResponse {
  code: ErrorCode;
  message: string;
}

/** State of the web server, of a PHP-FPM master or of a service server. */
export type ProcessState =
  | { state: "stopped" }
  | { state: "starting" }
  | { state: "running"; pid: number }
  | { state: "failed"; message: string };

export type DnsServerState =
  | { state: "listening" }
  | { state: "failed"; message: string };

export type IntegrationStatus =
  | "installed"
  | "missing"
  | "outdated"
  | "conflict"
  | "unsupported"
  | "unknown";

export interface SystemIntegrations {
  resolver: IntegrationStatus;
  port_forwarding: IntegrationStatus;
  certificate_trust: IntegrationStatus;
  forwarding_active: boolean | null;
}

export type SiteTarget =
  | { type: "static"; root: string }
  | { type: "php"; root: string; version: string }
  | { type: "proxy"; upstream: string }
  | { type: "unavailable"; reason: string };

export interface SiteInfo {
  project: string;
  domain: string;
  target: SiteTarget;
}

export interface WebStatus {
  caddy_version: string;
  caddy_installed: boolean;
  server: ProcessState;
  tld: string;
  address: string;
  http_port: number;
  https_port: number;
  dns_port: number;
  dns: DnsServerState;
  certificate_authority: { certificate_pem: string; sha256: string } | null;
  system: SystemIntegrations;
  sites: SiteInfo[];
  php: { version: string; state: ProcessState }[];
}

export interface RuntimeVersionInfo {
  name: string;
  version: string;
  available: boolean;
  path: string | null;
}

export interface RuntimeList {
  index_serial: number;
  platform: string;
  versions: RuntimeVersionInfo[];
}

export interface RuntimeInfo {
  name: string;
  version: string;
  path: string;
  changed: boolean;
}

export interface RuntimeEnv {
  project: string | null;
  runtimes: {
    name: string;
    version: string;
    bin_dirs: string[];
    variables: [string, string][];
  }[];
  missing: string[];
}

export type ServiceMode = "automatic" | "started" | "stopped";

export interface ServiceInstanceInfo {
  name: string;
  series: string;
  version: string | null;
  port: number;
  user: string;
  mode: ServiceMode;
  required_by: string[];
  state: ProcessState;
  blocked: string | null;
  data_dir: string;
  log_file: string;
}

export interface ServiceList {
  address: string;
  instances: ServiceInstanceInfo[];
  missing: string[];
}

/** `DaemonView` of the desktop shell. */
export type DaemonView =
  | ({ state: "running" } & DaemonStatus)
  | { state: "stopped"; last_error: string | null }
  | { state: "starting" }
  | { state: "stopping" }
  | { state: "unknown"; message: string };
