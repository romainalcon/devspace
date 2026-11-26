<script setup lang="ts">
// A command for the user to run in a terminal (privileged operations stay
// in the CLI).
import { Check, Copy } from "lucide-vue-next";
import { ref } from "vue";

import { Button } from "@/components/ui/button";

const props = defineProps<{ command: string }>();
const copied = ref(false);

async function copy() {
  try {
    await navigator.clipboard.writeText(props.command);
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    copied.value = false;
  }
}
</script>

<template>
  <div class="bg-muted flex items-center justify-between gap-2 rounded-md py-1 pr-1 pl-3">
    <code class="text-xs">{{ command }}</code>
    <Button variant="ghost" size="icon-xs" :aria-label="`Copy ${command}`" @click="copy">
      <Check v-if="copied" class="text-success" />
      <Copy v-else />
    </Button>
  </div>
</template>
