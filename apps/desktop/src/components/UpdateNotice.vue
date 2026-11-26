<script setup lang="ts">
// Checks for an update of the application when it starts and every six
// hours, and offers to install it (the daemon is stopped, the application
// restarts).
import { Download } from "lucide-vue-next";
import { onBeforeUnmount, onMounted, ref } from "vue";
import { toast } from "vue-sonner";

import { Button } from "@/components/ui/button";
import { errorMessage, updates, type UpdateInfo } from "@/lib/daemon";

const CHECK_PERIOD_MS = 6 * 60 * 60 * 1000;

const update = ref<UpdateInfo | null>(null);
const installing = ref(false);

async function check() {
  try {
    update.value = await updates.check();
  } catch (error) {
    // Offline, or no release published yet: nothing to offer.
    console.warn("update check failed:", errorMessage(error));
  }
}

async function install() {
  installing.value = true;
  const id = toast.loading(`Installing DevSpace ${update.value?.version}…`, {
    duration: Number.POSITIVE_INFINITY,
  });
  try {
    await updates.install();
  } catch (error) {
    toast.error("Update failed", {
      id,
      description: errorMessage(error),
      duration: Number.POSITIVE_INFINITY,
      closeButton: true,
    });
    installing.value = false;
  }
}

let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  void check();
  timer = setInterval(() => void check(), CHECK_PERIOD_MS);
});
onBeforeUnmount(() => clearInterval(timer));
</script>

<template>
  <div v-if="update" class="space-y-2 rounded-md border p-3 text-xs">
    <p>
      <span class="font-medium">DevSpace {{ update.version }}</span>
      is available (you have {{ update.current_version }}).
    </p>
    <p class="text-muted-foreground">Installing stops the daemon, then restarts DevSpace.</p>
    <Button size="sm" class="w-full" :disabled="installing" @click="install">
      <Download /> {{ installing ? "Installing…" : "Install and restart" }}
    </Button>
  </div>
</template>
