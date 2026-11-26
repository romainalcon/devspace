<script setup lang="ts">
// Published and installed versions of some packages, with install and
// remove actions (`devspace runtime install|remove`).
import { Download, Loader2, Trash2 } from "lucide-vue-next";
import { computed } from "vue";

import StatusBadge from "@/components/StatusBadge.vue";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { isPending, runAction } from "@/lib/actions";
import { request } from "@/lib/daemon";
import { packageName } from "@/lib/format";
import type { RuntimeVersionInfo } from "@/lib/protocol";
import { store } from "@/lib/store";

const props = defineProps<{
  /** Packages to show, in this order. */
  include?: readonly string[];
  /** Packages not to show. */
  exclude?: readonly string[];
}>();

const groups = computed(() => {
  const list = store.runtimes;
  if (!list) return null;
  const byName = new Map<string, RuntimeVersionInfo[]>();
  for (const version of list.versions) {
    if (props.include && !props.include.includes(version.name)) continue;
    if (props.exclude?.includes(version.name)) continue;
    byName.set(version.name, [...(byName.get(version.name) ?? []), version]);
  }
  const names = [...byName.keys()];
  if (props.include) names.sort((a, b) => props.include!.indexOf(a) - props.include!.indexOf(b));
  // Newest first; the daemon sorts by version, oldest first.
  return names.map((name) => ({ name, versions: [...byName.get(name)!].reverse() }));
});

const key = (v: RuntimeVersionInfo) => `package:${v.name}:${v.version}`;

function install(v: RuntimeVersionInfo) {
  void runAction(
    key(v),
    {
      running: `Installing ${packageName(v.name)} ${v.version}…`,
      done: `${packageName(v.name)} ${v.version} installed`,
    },
    () => request({ method: "runtime_install", name: v.name, version: v.version }, "runtime_installed"),
  );
}

function remove(v: RuntimeVersionInfo) {
  void runAction(
    key(v),
    {
      running: `Removing ${packageName(v.name)} ${v.version}…`,
      done: `${packageName(v.name)} ${v.version} removed`,
    },
    () => request({ method: "runtime_remove", name: v.name, version: v.version }, "runtime_removed"),
  );
}
</script>

<template>
  <p v-if="store.errors.runtimes" class="text-destructive selectable mb-4 text-sm">
    {{ store.errors.runtimes }}
  </p>
  <div v-if="groups" class="grid gap-4 lg:grid-cols-2">
    <Card v-for="group in groups" :key="group.name">
      <CardHeader>
        <CardTitle>{{ packageName(group.name) }}</CardTitle>
        <CardDescription>
          {{ group.versions.filter((v) => v.path).length }} installed ·
          {{ group.versions.filter((v) => v.available).length }} published for
          {{ store.runtimes?.platform }}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ul class="divide-y">
          <li
            v-for="v in group.versions"
            :key="v.version"
            class="flex items-center justify-between gap-4 py-2 first:pt-0 last:pb-0"
          >
            <div class="min-w-0">
              <p class="selectable text-sm font-medium">{{ v.version }}</p>
              <p v-if="v.path" class="text-muted-foreground selectable truncate text-xs" :title="v.path">
                {{ v.path }}
              </p>
            </div>
            <div class="flex shrink-0 items-center gap-2">
              <StatusBadge v-if="v.path && !v.available" tone="neutral" label="No longer published" />
              <StatusBadge v-else-if="v.path" tone="success" label="Installed" />
              <Loader2 v-if="isPending(key(v))" class="text-muted-foreground size-4 animate-spin" />
              <Button
                v-else-if="v.path"
                variant="ghost"
                size="icon-sm"
                :title="`Remove ${v.version}`"
                @click="remove(v)"
              >
                <Trash2 />
              </Button>
              <Button v-else variant="outline" size="sm" @click="install(v)">
                <Download /> Install
              </Button>
            </div>
          </li>
        </ul>
      </CardContent>
    </Card>
  </div>
  <div v-else-if="!store.errors.runtimes" class="grid gap-4 lg:grid-cols-2">
    <Skeleton class="h-48 rounded-xl" />
    <Skeleton class="h-48 rounded-xl" />
  </div>
</template>
