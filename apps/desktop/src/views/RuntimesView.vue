<script setup lang="ts">
import { RefreshCw } from "lucide-vue-next";

import DaemonGate from "@/components/DaemonGate.vue";
import PackageVersions from "@/components/PackageVersions.vue";
import PageHeader from "@/components/PageHeader.vue";
import { Button } from "@/components/ui/button";
import { isPending, runAction } from "@/lib/actions";
import { request } from "@/lib/daemon";
import { SERVICE_PACKAGES } from "@/lib/format";
import { store } from "@/lib/store";

function refreshIndex() {
  void runAction(
    "index",
    {
      running: "Checking for new versions…",
      done: (list) =>
        list.index_error
          ? `Could not download the published index: ${list.index_error}`
          : `Package index #${list.index_serial} (${list.index_source})`,
    },
    () => request({ method: "index_refresh" }, "runtimes"),
  );
}
</script>

<template>
  <PageHeader
    title="Runtimes"
    :description="`Language runtimes from the signed package index${store.runtimes ? ` #${store.runtimes.index_serial} (${store.runtimes.index_source})` : ''}. Projects choose theirs in devspace.toml, else the newest installed one is used.`"
  >
    <Button
      v-if="store.runtimes"
      variant="outline"
      size="sm"
      :disabled="isPending('index')"
      @click="refreshIndex"
    >
      <RefreshCw /> Check for new versions
    </Button>
  </PageHeader>
  <DaemonGate>
    <PackageVersions :exclude="SERVICE_PACKAGES" />
  </DaemonGate>
</template>
