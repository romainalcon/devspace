<script setup lang="ts">
import { Download, Play, Power, RotateCw, Square } from "lucide-vue-next";
import { computed } from "vue";

import CopyCommand from "@/components/CopyCommand.vue";
import DaemonGate from "@/components/DaemonGate.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { isPending, runAction } from "@/lib/actions";
import { daemon, request } from "@/lib/daemon";
import { formatUptime, integrationLabel, processLabel } from "@/lib/format";
import type { IntegrationStatus } from "@/lib/protocol";
import { store } from "@/lib/store";

const web = computed(() => store.web);
const running = computed(() => (store.daemon?.state === "running" ? store.daemon : null));

const integrations = computed(() => {
  const status = web.value;
  if (!status) return [];
  const system = status.system;
  const forwarding: { label: string; tone: "warning" } | null =
    system.port_forwarding === "installed" && system.forwarding_active === false
      ? { label: "Inactive", tone: "warning" }
      : null;
  return [
    {
      name: "Local domains",
      detail: `*.${status.tld} resolved by DevSpace`,
      badge: integrationLabel(system.resolver),
    },
    {
      name: "Ports 80 and 443",
      detail: `${status.address} forwarded to the web server`,
      badge: forwarding ?? integrationLabel(system.port_forwarding),
    },
    {
      name: "HTTPS certificates",
      detail: "Local certificate authority trusted by the system",
      badge: integrationLabel(system.certificate_trust),
    },
  ];
});

const needsSetup = computed(() => {
  const system = web.value?.system;
  if (!system) return false;
  const actionable: IntegrationStatus[] = ["missing", "outdated"];
  return [system.resolver, system.port_forwarding, system.certificate_trust].some((s) =>
    actionable.includes(s),
  );
});

const webBusy = computed(() => isPending("web"));

function webAction(method: "web_install" | "web_start" | "web_stop" | "web_restart") {
  const messages = {
    web_install: { running: "Installing Caddy…", done: "Caddy installed" },
    web_start: { running: "Starting the web server…", done: "Web server started" },
    web_stop: { running: "Stopping the web server…", done: "Web server stopped" },
    web_restart: { running: "Restarting the web server…", done: "Web server restarted" },
  }[method];
  void runAction("web", messages, () => request({ method }, "web_status"));
}

function stopDaemon() {
  void runAction("daemon", { running: "Stopping the daemon…", done: "Daemon stopped" }, daemon.stop);
}
</script>

<template>
  <PageHeader title="Overview" description="The daemon, the web server and the local domains.">
    <Button
      v-if="running"
      variant="outline"
      size="sm"
      :disabled="isPending('daemon')"
      @click="stopDaemon"
    >
      <Power /> Stop daemon
    </Button>
  </PageHeader>

  <DaemonGate>
    <div class="grid gap-4 lg:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle class="flex items-center justify-between">
            Daemon
            <StatusBadge tone="success" label="Running" />
          </CardTitle>
          <CardDescription>Runs the sites and services, also without this window.</CardDescription>
        </CardHeader>
        <CardContent v-if="running">
          <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
            <dt class="text-muted-foreground">Version</dt>
            <dd class="selectable">{{ running.version }}</dd>
            <dt class="text-muted-foreground">Process</dt>
            <dd class="selectable">{{ running.pid }}</dd>
            <dt class="text-muted-foreground">Uptime</dt>
            <dd>{{ formatUptime(running.uptime_secs) }}</dd>
          </dl>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle class="flex items-center justify-between">
            Web server
            <template v-if="web">
              <StatusBadge
                v-if="web.caddy_installed"
                v-bind="processLabel(web.server)"
              />
              <StatusBadge v-else tone="warning" label="Not installed" />
            </template>
          </CardTitle>
          <CardDescription>
            Caddy {{ web?.caddy_version }} serving
            <code v-if="web">https://*.{{ web.tld }}</code>
          </CardDescription>
        </CardHeader>
        <CardContent v-if="web" class="space-y-4">
          <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
            <dt class="text-muted-foreground">Address</dt>
            <dd class="selectable">
              {{ web.address }} (HTTP {{ web.http_port }}, HTTPS {{ web.https_port }})
            </dd>
            <dt class="text-muted-foreground">DNS server</dt>
            <dd>
              <span v-if="web.dns.state === 'listening'">127.0.0.1:{{ web.dns_port }}</span>
              <span v-else class="text-destructive selectable">{{ web.dns.message }}</span>
            </dd>
            <template v-if="web.certificate_authority">
              <dt class="text-muted-foreground">Local CA</dt>
              <dd class="selectable font-mono text-xs break-all">
                {{ web.certificate_authority.sha256 }}
              </dd>
            </template>
          </dl>
          <p
            v-if="web.server.state === 'failed'"
            class="bg-destructive/10 text-destructive selectable rounded-md p-3 text-xs whitespace-pre-wrap"
          >
            {{ web.server.message }}
          </p>
          <div class="flex flex-wrap gap-2">
            <Button
              v-if="!web.caddy_installed"
              size="sm"
              :disabled="webBusy"
              @click="webAction('web_install')"
            >
              <Download /> Install Caddy
            </Button>
            <template v-else>
              <Button
                v-if="web.server.state !== 'running'"
                size="sm"
                :disabled="webBusy"
                @click="webAction('web_start')"
              >
                <Play /> Start
              </Button>
              <Button
                v-else
                size="sm"
                variant="outline"
                :disabled="webBusy"
                @click="webAction('web_stop')"
              >
                <Square /> Stop
              </Button>
              <Button
                size="sm"
                variant="outline"
                :disabled="webBusy"
                @click="webAction('web_restart')"
              >
                <RotateCw /> Restart
              </Button>
            </template>
          </div>
        </CardContent>
        <CardContent v-else-if="store.errors.web">
          <p class="text-destructive selectable text-sm whitespace-pre-wrap">{{ store.errors.web }}</p>
        </CardContent>
        <CardContent v-else class="space-y-2">
          <Skeleton class="h-4 w-2/3" />
          <Skeleton class="h-4 w-1/2" />
        </CardContent>
      </Card>

      <Card class="lg:col-span-2">
        <CardHeader>
          <CardTitle>System integrations</CardTitle>
          <CardDescription>
            Machine-wide changes that make the local domains reachable. They need administrator
            rights and are made by the command line.
          </CardDescription>
        </CardHeader>
        <CardContent v-if="web" class="space-y-4">
          <ul class="divide-y">
            <li
              v-for="item in integrations"
              :key="item.name"
              class="flex items-center justify-between gap-4 py-2.5 first:pt-0 last:pb-0"
            >
              <div>
                <p class="text-sm font-medium">{{ item.name }}</p>
                <p class="text-muted-foreground text-xs">{{ item.detail }}</p>
              </div>
              <StatusBadge v-bind="item.badge" />
            </li>
          </ul>
          <Alert v-if="needsSetup">
            <AlertTitle>Setup needed</AlertTitle>
            <AlertDescription class="space-y-2">
              <p>Run this in a terminal; it shows each change and asks for your approval:</p>
              <CopyCommand command="devspace system setup" />
            </AlertDescription>
          </Alert>
        </CardContent>
      </Card>

      <Card v-if="web && web.php.length > 0" class="lg:col-span-2">
        <CardHeader>
          <CardTitle>PHP process managers</CardTitle>
          <CardDescription>One PHP-FPM per version used by the PHP sites.</CardDescription>
        </CardHeader>
        <CardContent>
          <ul class="divide-y">
            <li
              v-for="pool in web.php"
              :key="pool.version"
              class="flex items-center justify-between gap-4 py-2.5 first:pt-0 last:pb-0"
            >
              <span class="text-sm">PHP {{ pool.version }}</span>
              <div class="flex items-center gap-3">
                <span
                  v-if="pool.state.state === 'failed'"
                  class="text-destructive selectable max-w-md truncate text-xs"
                  :title="pool.state.message"
                  >{{ pool.state.message }}</span
                >
                <StatusBadge v-bind="processLabel(pool.state)" />
              </div>
            </li>
          </ul>
        </CardContent>
      </Card>
    </div>
  </DaemonGate>
</template>
