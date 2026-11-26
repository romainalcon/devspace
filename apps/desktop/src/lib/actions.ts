// Actions sent to the daemon: one at a time per key, with a notification
// while they run (installations take minutes) and when they end.

import { reactive } from "vue";
import { toast } from "vue-sonner";

import { errorMessage } from "@/lib/daemon";
import { refresh } from "@/lib/store";

const pending = reactive(new Set<string>());

/** Whether the action `key` is running. */
export function isPending(key: string): boolean {
  return pending.has(key);
}

/**
 * Runs `action` unless the action `key` already runs, then refreshes the
 * snapshot. Failures are shown, not thrown; returns whether it succeeded.
 */
export async function runAction<T>(
  key: string,
  messages: { running: string; done: string | ((result: T) => string) },
  action: () => Promise<T>,
): Promise<boolean> {
  if (pending.has(key)) return false;
  pending.add(key);
  const id = toast.loading(messages.running, { duration: Number.POSITIVE_INFINITY });
  try {
    const result = await action();
    const done = typeof messages.done === "function" ? messages.done(result) : messages.done;
    toast.success(done, { id, duration: 4_000 });
    return true;
  } catch (error) {
    toast.error(messages.running.replace(/…$/, " failed"), {
      id,
      description: errorMessage(error),
      duration: Number.POSITIVE_INFINITY,
      closeButton: true,
    });
    return false;
  } finally {
    pending.delete(key);
    await refresh();
  }
}
