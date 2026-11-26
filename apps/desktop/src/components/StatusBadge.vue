<script setup lang="ts">
import { computed } from "vue";

import type { Tone } from "@/lib/format";
import { cn } from "@/lib/utils";

const props = defineProps<{ tone: Tone; label: string }>();

const classes: Record<Tone, string> = {
  success: "bg-success/12 text-success border-success/25",
  warning: "bg-warning/15 text-warning border-warning/30",
  danger: "bg-destructive/12 text-destructive border-destructive/25",
  neutral: "bg-muted text-muted-foreground border-border",
};
const dot: Record<Tone, string> = {
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-destructive",
  neutral: "bg-muted-foreground/60",
};
const tone = computed(() => props.tone);
</script>

<template>
  <span
    :class="
      cn(
        'inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium whitespace-nowrap',
        classes[tone],
      )
    "
  >
    <span :class="cn('size-1.5 rounded-full', dot[tone])" />
    {{ label }}
  </span>
</template>
