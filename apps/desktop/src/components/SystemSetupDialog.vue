<script setup lang="ts">
// Runs `devspace system setup|teardown` from the application: the plan
// printed by the CLI (`--dry-run`) is shown first; once approved, the CLI
// applies it and the system asks for the administrator password.
import { ShieldCheck } from "lucide-vue-next";
import { ref, watch } from "vue";
import { toast } from "vue-sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { errorMessage, system, type SystemAction, type SystemOutput } from "@/lib/daemon";
import { refresh } from "@/lib/store";

const props = defineProps<{ action: SystemAction }>();
const open = defineModel<boolean>("open", { required: true });

const plan = ref<SystemOutput | null>(null);
const loadError = ref<string | null>(null);
const applying = ref(false);

const titles = {
  setup: "Set up the system integrations",
  teardown: "Remove the system integrations",
};

watch(open, async (isOpen) => {
  if (!isOpen) return;
  plan.value = null;
  loadError.value = null;
  try {
    plan.value = await system.plan(props.action);
  } catch (error) {
    loadError.value = errorMessage(error);
  }
});

const nothingToDo = (output: SystemOutput) => output.stdout.trim() === "nothing to do";

async function apply() {
  applying.value = true;
  try {
    const result = await system.apply(props.action);
    if (result.success) {
      toast.success("System integrations updated", { description: result.stdout.trim() });
      open.value = false;
    } else {
      plan.value = result;
    }
  } catch (error) {
    loadError.value = errorMessage(error);
  } finally {
    applying.value = false;
    await refresh();
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle>{{ titles[action] }}</DialogTitle>
        <DialogDescription>
          DevSpace makes these changes with administrator rights; the system asks for your
          password. Files of other tools are never modified.
        </DialogDescription>
      </DialogHeader>
      <p v-if="loadError" class="text-destructive selectable text-sm whitespace-pre-wrap">
        {{ loadError }}
      </p>
      <template v-else-if="plan">
        <pre
          v-if="plan.stdout.trim()"
          class="bg-muted selectable max-h-80 overflow-auto rounded-md p-3 text-xs whitespace-pre-wrap"
          >{{ plan.stdout.trim() }}</pre
        >
        <p
          v-if="plan.stderr.trim()"
          class="bg-destructive/10 text-destructive selectable rounded-md p-3 text-xs whitespace-pre-wrap"
        >
          {{ plan.stderr.trim() }}
        </p>
      </template>
      <p v-else class="text-muted-foreground text-sm">Computing the changes…</p>
      <DialogFooter>
        <Button variant="outline" @click="open = false">Cancel</Button>
        <Button
          :disabled="!plan || !plan.success || nothingToDo(plan) || applying"
          @click="apply"
        >
          <ShieldCheck /> {{ applying ? "Applying…" : "Approve and apply" }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
