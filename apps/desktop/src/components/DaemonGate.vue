<script setup lang="ts">
// Shows its content only while the daemon runs; otherwise what prevents it.
import { Loader2, Power, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";

import { Button } from "@/components/ui/button";
import { isPending, runAction } from "@/lib/actions";
import { daemon } from "@/lib/daemon";
import { store } from "@/lib/store";

const view = computed(() => store.daemon);

function start() {
  void runAction("daemon", { running: "Starting the daemon…", done: "Daemon started" }, daemon.start);
}
</script>

<template>
  <slot v-if="view?.state === 'running'" />
  <div
    v-else
    class="flex min-h-80 flex-col items-center justify-center gap-4 rounded-xl border border-dashed p-10 text-center"
  >
    <template v-if="view === null || view.state === 'starting'">
      <Loader2 class="text-muted-foreground size-6 animate-spin" />
      <p class="text-muted-foreground text-sm">Starting the DevSpace daemon…</p>
    </template>
    <template v-else-if="view.state === 'stopping'">
      <Loader2 class="text-muted-foreground size-6 animate-spin" />
      <p class="text-muted-foreground text-sm">Stopping the DevSpace daemon…</p>
    </template>
    <template v-else>
      <TriangleAlert v-if="view.state === 'unknown' || view.last_error" class="text-warning size-6" />
      <Power v-else class="text-muted-foreground size-6" />
      <div class="space-y-1">
        <p class="font-medium">
          {{ view.state === "unknown" ? "The daemon is not responding" : "The daemon is not running" }}
        </p>
        <p class="text-muted-foreground text-sm">
          The daemon runs your sites and services; this window only controls it.
        </p>
      </div>
      <pre
        v-if="view.state === 'unknown' || view.last_error"
        class="bg-muted text-muted-foreground max-w-2xl overflow-x-auto rounded-md p-3 text-left text-xs whitespace-pre-wrap"
        >{{ view.state === "unknown" ? view.message : view.last_error }}</pre
      >
      <Button v-if="view.state === 'stopped'" :disabled="isPending('daemon')" @click="start">
        <Power /> Start daemon
      </Button>
    </template>
  </div>
</template>
