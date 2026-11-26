<script setup lang="ts">
import { Boxes, Database, FolderGit2, LayoutDashboard } from "lucide-vue-next";
import type { Component } from "vue";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import "vue-sonner/style.css";

import StatusBadge from "@/components/StatusBadge.vue";
import UpdateNotice from "@/components/UpdateNotice.vue";
import { Toaster } from "@/components/ui/sonner";
import type { Tone } from "@/lib/format";
import { startPolling, store } from "@/lib/store";
import { cn } from "@/lib/utils";
import OverviewView from "@/views/OverviewView.vue";
import ProjectsView from "@/views/ProjectsView.vue";
import RuntimesView from "@/views/RuntimesView.vue";
import ServicesView from "@/views/ServicesView.vue";

type Page = "overview" | "projects" | "runtimes" | "services";

const pages: { id: Page; label: string; icon: Component; view: Component }[] = [
  { id: "overview", label: "Overview", icon: LayoutDashboard, view: OverviewView },
  { id: "projects", label: "Projects", icon: FolderGit2, view: ProjectsView },
  { id: "runtimes", label: "Runtimes", icon: Boxes, view: RuntimesView },
  { id: "services", label: "Services", icon: Database, view: ServicesView },
];

const current = ref<Page>("overview");
const view = computed(() => pages.find((p) => p.id === current.value)?.view ?? OverviewView);

const daemonBadge = computed((): { tone: Tone; label: string } => {
  switch (store.daemon?.state) {
    case "running":
      return { tone: "success", label: "Daemon running" };
    case "starting":
    case undefined:
      return { tone: "warning", label: "Daemon starting" };
    case "stopping":
      return { tone: "warning", label: "Daemon stopping" };
    case "stopped":
      return store.daemon.last_error
        ? { tone: "danger", label: "Daemon failed" }
        : { tone: "neutral", label: "Daemon stopped" };
    case "unknown":
      return { tone: "danger", label: "Daemon not responding" };
  }
});

let stopPolling: (() => void) | undefined;
onMounted(() => (stopPolling = startPolling()));
onBeforeUnmount(() => stopPolling?.());
</script>

<template>
  <div class="flex h-full">
    <aside class="bg-sidebar text-sidebar-foreground flex w-56 shrink-0 flex-col border-r">
      <div class="px-5 pt-6 pb-4">
        <p class="text-base font-semibold tracking-tight">DevSpace</p>
        <p class="text-muted-foreground text-xs">Local development environments</p>
      </div>
      <nav class="flex flex-1 flex-col gap-0.5 px-3">
        <button
          v-for="page in pages"
          :key="page.id"
          type="button"
          :class="
            cn(
              'flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-sm transition-colors',
              current === page.id
                ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
                : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-foreground',
            )
          "
          @click="current = page.id"
        >
          <component :is="page.icon" class="size-4" />
          {{ page.label }}
        </button>
      </nav>
      <div class="px-3 pb-3">
        <UpdateNotice />
      </div>
      <div class="border-t px-5 py-4">
        <StatusBadge v-bind="daemonBadge" />
      </div>
    </aside>

    <main class="min-w-0 flex-1 overflow-y-auto">
      <div class="mx-auto max-w-5xl px-8 py-8">
        <component :is="view" />
      </div>
    </main>
  </div>
  <Toaster position="bottom-right" rich-colors />
</template>
