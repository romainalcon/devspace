<script setup lang="ts">
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ExternalLink, FolderOpen, FolderPlus, Plus, Trash2 } from "lucide-vue-next";
import { computed, ref } from "vue";
import { toast } from "vue-sonner";

import DaemonGate from "@/components/DaemonGate.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusBadge from "@/components/StatusBadge.vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { isPending, runAction } from "@/lib/actions";
import { daemon, errorMessage, request } from "@/lib/daemon";
import { siteTargetLabel } from "@/lib/format";
import type { ProjectInfo, SiteInfo } from "@/lib/protocol";
import { store } from "@/lib/store";

const sites = computed(() => new Map((store.web?.sites ?? []).map((s) => [s.project, s])));
const webRunning = computed(() => store.web?.server.state === "running");

function site(project: ProjectInfo): SiteInfo | undefined {
  return sites.value.get(project.name);
}

// Adding a project.
const adding = ref(false);
const addPath = ref("");
const addName = ref("");
const addError = ref("");

function showAdd() {
  addPath.value = "";
  addName.value = "";
  addError.value = "";
  adding.value = true;
}

async function choose() {
  try {
    const picked = await openDialog({ directory: true, multiple: false, title: "Project directory" });
    if (typeof picked === "string") addPath.value = picked;
  } catch (error) {
    addError.value = errorMessage(error);
  }
}

async function add() {
  const path = addPath.value;
  const name = addName.value.trim() || undefined;
  const ok = await runAction(
    "project:add",
    { running: "Adding the project…", done: (p: ProjectInfo) => `Project ${p.name} added` },
    () => request({ method: "project_add", path, name }, "project_added"),
  );
  if (ok) adding.value = false;
}

// Removing a project.
const removing = ref<ProjectInfo | null>(null);

async function remove() {
  const project = removing.value;
  if (!project) return;
  removing.value = null;
  await runAction(
    `project:${project.name}`,
    { running: `Removing ${project.name}…`, done: `Project ${project.name} removed` },
    () => request({ method: "project_remove", name: project.name }, "project_removed"),
  );
}

function openSite(project: ProjectInfo) {
  void runAction(
    `open:${project.name}`,
    { running: `Opening ${project.name}…`, done: `Opened ${project.name} in the browser` },
    () => daemon.openSite(project.name),
  );
}

function reveal(project: ProjectInfo) {
  daemon.revealProject(project.name).catch((error) => {
    toast.error("Could not show the folder", { description: errorMessage(error) });
  });
}

function unavailableReason(site: SiteInfo | undefined): string | null {
  return site?.target.type === "unavailable" ? site.target.reason : null;
}
</script>

<template>
  <PageHeader
    title="Projects"
    description="Registered directories, each served on its own local domain."
  >
    <Button v-if="store.daemon?.state === 'running'" size="sm" @click="showAdd">
      <Plus /> Add project
    </Button>
  </PageHeader>

  <DaemonGate>
    <p v-if="store.errors.projects" class="text-destructive selectable mb-4 text-sm">
      {{ store.errors.projects }}
    </p>

    <div
      v-if="store.projects && store.projects.length === 0"
      class="flex min-h-64 flex-col items-center justify-center gap-3 rounded-xl border border-dashed p-10 text-center"
    >
      <FolderPlus class="text-muted-foreground size-6" />
      <p class="font-medium">No project yet</p>
      <p class="text-muted-foreground max-w-md text-sm">
        Add a directory to serve it on a local domain. DevSpace never modifies it; a
        <code>devspace.toml</code> file in it can choose its name, runtimes and services.
      </p>
      <Button size="sm" @click="showAdd"><Plus /> Add project</Button>
    </div>

    <div v-else-if="store.projects" class="rounded-xl border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Project</TableHead>
            <TableHead>Site</TableHead>
            <TableHead>Serves</TableHead>
            <TableHead class="w-0" />
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="project in store.projects" :key="project.name">
            <TableCell class="max-w-72">
              <p class="font-medium">{{ project.name }}</p>
              <p class="text-muted-foreground selectable truncate text-xs" :title="project.path">
                {{ project.path }}
              </p>
            </TableCell>
            <TableCell>
              <code v-if="site(project)" class="text-xs">https://{{ site(project)?.domain }}</code>
              <span v-else class="text-muted-foreground text-xs">…</span>
            </TableCell>
            <TableCell class="max-w-72">
              <template v-if="site(project)">
                <template v-if="unavailableReason(site(project)) !== null">
                  <StatusBadge tone="warning" label="Not served" />
                  <p class="text-muted-foreground selectable mt-1 text-xs whitespace-normal">
                    {{ unavailableReason(site(project)) }}
                  </p>
                </template>
                <span v-else class="text-sm">{{ siteTargetLabel(site(project)!.target) }}</span>
              </template>
            </TableCell>
            <TableCell>
              <div class="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  :disabled="!webRunning || site(project)?.target.type === 'unavailable'"
                  :title="webRunning ? 'Open in the browser' : 'The web server is not running'"
                  @click="openSite(project)"
                >
                  <ExternalLink />
                </Button>
                <Button variant="ghost" size="icon-sm" title="Show the folder" @click="reveal(project)">
                  <FolderOpen />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  title="Remove from DevSpace"
                  :disabled="isPending(`project:${project.name}`)"
                  @click="removing = project"
                >
                  <Trash2 />
                </Button>
              </div>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </DaemonGate>

  <Dialog v-model:open="adding">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Add a project</DialogTitle>
        <DialogDescription>
          The directory is registered as it is; DevSpace does not create or change files in it.
        </DialogDescription>
      </DialogHeader>
      <div class="space-y-4">
        <div class="space-y-2">
          <Label for="project-path">Directory</Label>
          <div class="flex gap-2">
            <Input id="project-path" v-model="addPath" placeholder="/path/to/project" />
            <Button variant="outline" @click="choose">Choose…</Button>
          </div>
        </div>
        <div class="space-y-2">
          <Label for="project-name">Name (optional)</Label>
          <Input
            id="project-name"
            v-model="addName"
            placeholder="From devspace.toml, else the directory name"
          />
        </div>
        <p v-if="addError" class="text-destructive text-sm">{{ addError }}</p>
      </div>
      <DialogFooter>
        <Button variant="outline" @click="adding = false">Cancel</Button>
        <Button :disabled="!addPath || isPending('project:add')" @click="add">Add project</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <Dialog :open="removing !== null" @update:open="(open) => !open && (removing = null)">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Remove {{ removing?.name }}?</DialogTitle>
        <DialogDescription>
          Its site stops being served. The directory and its files are kept.
        </DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button variant="outline" @click="removing = null">Cancel</Button>
        <Button variant="destructive" @click="remove">Remove</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
