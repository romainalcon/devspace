<script setup lang="ts">
import { Loader2, Play, Square } from "lucide-vue-next";
import { computed } from "vue";

import DaemonGate from "@/components/DaemonGate.vue";
import PackageVersions from "@/components/PackageVersions.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { isPending, runAction } from "@/lib/actions";
import { request } from "@/lib/daemon";
import { packageName, processLabel, SERVICE_PACKAGES } from "@/lib/format";
import type { ServiceInstanceInfo, ServiceMode } from "@/lib/protocol";
import { store } from "@/lib/store";

const services = computed(() => store.services);

const modeLabels: Record<ServiceMode, string> = {
  automatic: "When a project needs it",
  started: "Always (started by you)",
  stopped: "Never (stopped by you)",
};

const key = (i: ServiceInstanceInfo) => `service:${i.name}:${i.series}`;
const title = (i: ServiceInstanceInfo) => `${packageName(i.name)} ${i.series}`;
const isUp = (i: ServiceInstanceInfo) => i.state.state === "running" || i.state.state === "starting";

function start(i: ServiceInstanceInfo) {
  void runAction(
    key(i),
    { running: `Starting ${title(i)}…`, done: `${title(i)} started` },
    () => request({ method: "service_start", name: i.name, series: i.series }, "services"),
  );
}

function stop(i: ServiceInstanceInfo) {
  void runAction(
    key(i),
    { running: `Stopping ${title(i)}…`, done: `${title(i)} stopped` },
    () => request({ method: "service_stop", name: i.name, series: i.series }, "services"),
  );
}
</script>

<template>
  <PageHeader
    title="Services"
    description="Database servers shared by every project, one per series, without a password, on the loopback address."
  />

  <DaemonGate>
    <div class="space-y-8">
      <section class="space-y-4">
        <p v-if="store.errors.services" class="text-destructive selectable text-sm">
          {{ store.errors.services }}
        </p>
        <Alert v-if="services && services.missing.length > 0">
          <AlertTitle>Missing versions</AlertTitle>
          <AlertDescription>
            <ul class="selectable list-disc space-y-1 pl-4">
              <li v-for="message in services.missing" :key="message">{{ message }}</li>
            </ul>
          </AlertDescription>
        </Alert>

        <div
          v-if="services && services.instances.length === 0"
          class="text-muted-foreground rounded-xl border border-dashed p-8 text-center text-sm"
        >
          No service yet. Install PostgreSQL, MySQL or Valkey below, or require one in a project's
          <code>devspace.toml</code> (<code>[services] postgresql = "18"</code>).
        </div>

        <div v-else-if="services" class="rounded-xl border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Service</TableHead>
                <TableHead>Connection</TableHead>
                <TableHead>Runs</TableHead>
                <TableHead>State</TableHead>
                <TableHead class="w-0" />
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="instance in services.instances" :key="key(instance)">
                <TableCell>
                  <p class="font-medium">{{ title(instance) }}</p>
                  <p class="text-muted-foreground selectable text-xs">
                    {{ instance.version ?? "not installed" }}
                  </p>
                </TableCell>
                <TableCell>
                  <code class="text-xs">{{ services.address }}:{{ instance.port }}</code>
                  <p class="text-muted-foreground selectable text-xs">user {{ instance.user }}</p>
                </TableCell>
                <TableCell class="max-w-64 whitespace-normal">
                  <p class="text-sm">{{ modeLabels[instance.mode] }}</p>
                  <p v-if="instance.required_by.length > 0" class="text-muted-foreground text-xs">
                    Needed by {{ instance.required_by.join(", ") }}
                  </p>
                </TableCell>
                <TableCell class="max-w-72 whitespace-normal">
                  <StatusBadge v-bind="processLabel(instance.state)" />
                  <p
                    v-if="instance.blocked"
                    class="text-warning selectable mt-1 text-xs"
                  >
                    {{ instance.blocked }}
                  </p>
                  <p
                    v-if="instance.state.state === 'failed'"
                    class="text-destructive selectable mt-1 text-xs"
                  >
                    {{ instance.state.message }}
                  </p>
                </TableCell>
                <TableCell>
                  <div class="flex justify-end">
                    <Loader2
                      v-if="isPending(key(instance))"
                      class="text-muted-foreground m-2 size-4 animate-spin"
                    />
                    <Button
                      v-else-if="isUp(instance)"
                      variant="outline"
                      size="sm"
                      @click="stop(instance)"
                    >
                      <Square /> Stop
                    </Button>
                    <Button
                      v-else
                      variant="outline"
                      size="sm"
                      :disabled="!instance.version"
                      @click="start(instance)"
                    >
                      <Play /> Start
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </section>

      <section class="space-y-4">
        <h2 class="text-base font-semibold">Versions</h2>
        <PackageVersions :include="SERVICE_PACKAGES" />
      </section>
    </div>
  </DaemonGate>
</template>
