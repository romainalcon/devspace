<script setup lang="ts">
import DaemonGate from "@/components/DaemonGate.vue";
import PackageVersions from "@/components/PackageVersions.vue";
import PageHeader from "@/components/PageHeader.vue";
import { SERVICE_PACKAGES } from "@/lib/format";
import { store } from "@/lib/store";
</script>

<template>
  <PageHeader
    title="Runtimes"
    :description="`Language runtimes from the signed package index${store.runtimes ? ` #${store.runtimes.index_serial}` : ''}. Projects choose theirs in devspace.toml, else the newest installed one is used.`"
  />
  <DaemonGate>
    <PackageVersions :exclude="SERVICE_PACKAGES" />
  </DaemonGate>
</template>
