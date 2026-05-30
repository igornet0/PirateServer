import {
  Download,
  FileUp,
  FolderPlus,
  Pencil,
  PackageOpen,
  RefreshCw,
  Trash2,
  Folder,
  File as FileIcon,
  Database,
  Server,
  HardDrive,
} from "lucide-react";
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { DatabasesPanel, type HostDatabaseInstance } from "./DatabasesPanel";
import { DbExplorerPanel } from "./dbExplorer/DbExplorerPanel";
import { useI18n } from "./i18n";
import { ModalDialog } from "./ui/ModalDialog";

type StorageEntry = {
  name: string;
  path: string;
  kind: string;
  size: number;
  mtime_ms: number;
};

type StorageList = {
  path: string;
  entries: StorageEntry[];
};

type StorageUsage = {
  used_bytes: number;
  max_bytes: number;
  free_bytes?: number | null;
  used_percent?: number | null;
};

type StorageBindMountCandidate = {
  mount_point: string;
  fstype: string;
  source: string;
  avail_bytes?: number | null;
  total_bytes?: number | null;
};

type StorageBindActive = {
  volume: string;
  source: string;
  mount_point: string;
};

type StorageBindSourcesView = {
  candidates: StorageBindMountCandidate[];
  active_binds: StorageBindActive[];
};

function isMountBoundAsSource(mount: string, active: StorageBindActive[]): boolean {
  const m = mount.replace(/\/+$/, "") || mount;
  return active.some((b) => {
    const s = b.source.replace(/\/+$/, "") || b.source;
    return s === m;
  });
}

type UploadPlanItem = {
  local: string;
  remote: string;
  name: string;
  exists: boolean;
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(2)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function isStorageArchiveName(name: string): boolean {
  const lower = name.toLowerCase();
  return (
    lower.endsWith(".zip") || lower.endsWith(".tar") || lower.endsWith(".tar.gz") || lower.endsWith(".tgz")
  );
}

type ParsedHttpErr = {
  status: number;
  body?: { error?: { code?: string; conflict_path?: string; message?: string } };
};

function parseStorageHttpError(msg: string): ParsedHttpErr | null {
  const m = msg.match(/HTTP (\d+):\s*([\s\S]+)/);
  if (!m) return null;
  const status = parseInt(m[1] ?? "0", 10);
  const rest = (m[2] ?? "").trim();
  try {
    const body = JSON.parse(rest) as ParsedHttpErr["body"];
    return { status, body: body as ParsedHttpErr["body"] };
  } catch {
    return { status };
  }
}

/**
 * One file/folder row. Memoized so it only re-renders when its own `entry`
 * or `selected` flag changes — not on every unrelated StoragePanel state
 * update (rename input, modals, uploads, …). All callbacks passed in are
 * stable (`useCallback`), which is what makes the `React.memo` effective.
 */
const StorageRow = React.memo(function StorageRow({
  entry,
  selected,
  onRange,
  onToggle,
  onSingle,
  onOpen,
}: {
  entry: StorageEntry;
  selected: boolean;
  onRange: (e: StorageEntry) => void;
  onToggle: (e: StorageEntry) => void;
  onSingle: (e: StorageEntry) => void;
  onOpen: (e: StorageEntry) => void;
}) {
  return (
    <tr
      className={`cursor-pointer border-b border-border-subtle/50 hover:bg-white/5 ${
        selected ? "bg-red-950/20" : ""
      }`}
      onClick={(ev) => {
        if (ev.shiftKey) {
          onRange(entry);
        } else if (ev.metaKey || ev.ctrlKey) {
          onToggle(entry);
        } else {
          onSingle(entry);
        }
      }}
      onDoubleClick={() => onOpen(entry)}
    >
      <td className="px-3 py-1.5">
        <input
          type="checkbox"
          className="rounded border border-white/20"
          checked={selected}
          onChange={() => onToggle(entry)}
          onClick={(ev) => ev.stopPropagation()}
        />
      </td>
      <td className="px-3 py-1.5 font-mono text-slate-200">
        <span className="inline-flex items-center gap-1.5">
          {entry.kind === "dir" ? (
            <Folder className="h-3.5 w-3.5 text-amber-500/80" />
          ) : (
            <FileIcon className="h-3.5 w-3.5 text-slate-500" />
          )}
          {entry.name}
        </span>
      </td>
      <td className="px-3 py-1.5 text-slate-500">{entry.kind}</td>
      <td className="px-3 py-1.5 font-mono text-slate-500">
        {entry.kind === "file" ? formatBytes(entry.size) : "—"}
      </td>
    </tr>
  );
});

export function StoragePanel() {
  const { t, language } = useI18n();
  const tr = (ru: string, en: string) => (language === "ru" ? ru : en);
  const tauri = isTauri();
  const [path, setPath] = useState("");
  const [list, setList] = useState<StorageList | null>(null);
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [selected, setSelected] = useState<StorageEntry | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [lastClickedPath, setLastClickedPath] = useState<string | null>(null);
  const [extractBusy, setExtractBusy] = useState(false);
  const [extractConflict, setExtractConflict] = useState<{ path: string } | null>(null);
  const [extractOkHint, setExtractOkHint] = useState<string | null>(null);
  const [extractWarnings, setExtractWarnings] = useState<string[] | null>(null);

  const [newFolderOpen, setNewFolderOpen] = useState(false);
  const [newFolderName, setNewFolderName] = useState("");
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const [moveOpen, setMoveOpen] = useState(false);
  const [moveTargetDir, setMoveTargetDir] = useState("");
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleteRecursive, setDeleteRecursive] = useState(false);
  const [uploadBusy, setUploadBusy] = useState(false);
  const [uploadConflictOpen, setUploadConflictOpen] = useState(false);
  const [pendingUploadPlan, setPendingUploadPlan] = useState<UploadPlanItem[]>([]);

  const [storageView, setStorageView] = useState<"files" | "databases">("files");
  /** Databases sub-area: control-api host vs local Tauri direct. */
  const [storageDbMode, setStorageDbMode] = useState<"host" | "direct">("direct");
  const [hostDbList, setHostDbList] = useState<{ instances: HostDatabaseInstance[] } | null>(null);
  const [hostDbLoading, setHostDbLoading] = useState(false);

  const [bindView, setBindView] = useState<StorageBindSourcesView | null>(null);
  const [bindBusy, setBindBusy] = useState(false);
  const [bindModalOpen, setBindModalOpen] = useState(false);
  const [bindModalSource, setBindModalSource] = useState<string | null>(null);
  const [bindModalVolume, setBindModalVolume] = useState("");

  const reloadHostDatabases = useCallback(async () => {
    if (!tauri) return;
    setHostDbLoading(true);
    try {
      const j = await invoke<string>("control_api_host_databases_list_json");
      setHostDbList(JSON.parse(j) as { instances: HostDatabaseInstance[] });
    } catch {
      setHostDbList({ instances: [] });
    } finally {
      setHostDbLoading(false);
    }
  }, [tauri]);

  useEffect(() => {
    if (!tauri) {
      return;
    }
    void reloadHostDatabases();
  }, [tauri, reloadHostDatabases]);

  const hasHostDatabases = Boolean(hostDbList && hostDbList.instances.length > 0);

  const refresh = useCallback(async (pathToList?: string) => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      setList(null);
      return;
    }
    const p = pathToList ?? path;
    setLoading(true);
    setErr(null);
    try {
      const [treeJson, usageJson] = await Promise.all([
        invoke<string>("control_api_storage_tree_json", { path: p }),
        invoke<string>("control_api_storage_usage_json"),
      ]);
      const nextList = JSON.parse(treeJson) as StorageList;
      setList(nextList);
      setUsage(JSON.parse(usageJson) as StorageUsage);
      setSelectedPaths((prev) => {
        if (prev.size === 0) return prev;
        const present = new Set(nextList.entries.map((e) => e.path));
        const next = new Set<string>();
        prev.forEach((p) => {
          if (present.has(p)) next.add(p);
        });
        return next;
      });
      try {
        const bj = await invoke<string>("control_api_storage_bind_sources_json");
        setBindView(JSON.parse(bj) as StorageBindSourcesView);
      } catch {
        setBindView({ candidates: [], active_binds: [] });
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setList(null);
    } finally {
      setLoading(false);
    }
  }, [path, tauri, t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const parentPath = () => {
    if (!path) return "";
    const parts = path.split("/").filter(Boolean);
    parts.pop();
    return parts.join("/");
  };

  // Stable (only state setters in body) so memoized rows are not invalidated.
  const enter = useCallback((e: StorageEntry) => {
    if (e.kind === "dir") {
      setPath(e.path);
      setSelected(null);
      setSelectedPaths(new Set());
      setLastClickedPath(null);
    }
  }, []);

  const selectedEntries = useMemo(() => {
    if (!list) return [];
    return list.entries.filter((e) => selectedPaths.has(e.path));
  }, [list, selectedPaths]);
  const hasSelectedDirs = selectedEntries.some((e) => e.kind === "dir");
  const selectedCount = selectedEntries.length;
  const hasExactlyOneSelected = selectedCount === 1;

  const setSingleSelection = useCallback((entry: StorageEntry) => {
    setSelected(entry);
    setSelectedPaths(new Set([entry.path]));
    setLastClickedPath(entry.path);
  }, []);

  const toggleSelection = useCallback((entry: StorageEntry) => {
    setSelected(entry);
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      return next;
    });
    setLastClickedPath(entry.path);
  }, []);

  const selectRange = useCallback((entry: StorageEntry) => {
    if (!list || !lastClickedPath) {
      setSingleSelection(entry);
      return;
    }
    const from = list.entries.findIndex((it) => it.path === lastClickedPath);
    const to = list.entries.findIndex((it) => it.path === entry.path);
    if (from < 0 || to < 0) {
      setSingleSelection(entry);
      return;
    }
    const start = Math.min(from, to);
    const end = Math.max(from, to);
    const next = new Set<string>();
    for (let i = start; i <= end; i += 1) {
      next.add(list.entries[i]!.path);
    }
    setSelected(entry);
    setSelectedPaths(next);
  }, [lastClickedPath, list, setSingleSelection]);

  const openNewFolder = () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    setNewFolderName("");
    setNewFolderOpen(true);
  };

  const submitNewFolder = async () => {
    const name = newFolderName.trim();
    if (!name) {
      setErr(t("storage.nameRequired"));
      return;
    }
    const rel = path ? `${path}/${name}` : name;
    setErr(null);
    try {
      await invoke("control_api_storage_create_folder", { path: rel });
      setNewFolderOpen(false);
      setNewFolderName("");
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const openRename = () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    if (!selected || !hasExactlyOneSelected) return;
    setRenameValue(selected.path);
    setRenameOpen(true);
  };

  const submitRename = async () => {
    if (!selected) return;
    const to = renameValue.trim();
    if (to === "" || to === selected.path) {
      setRenameOpen(false);
      return;
    }
    setErr(null);
    try {
      await invoke("control_api_storage_rename", { from: selected.path, to });
      setRenameOpen(false);
      setSelected(null);
      const parent = to.split("/").slice(0, -1).join("/");
      setPath(parent);
      await refresh(parent);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const openDelete = () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    if (selectedCount === 0) return;
    setDeleteRecursive(false);
    setDeleteOpen(true);
  };

  const submitDelete = async () => {
    if (selectedEntries.length === 0) {
      setDeleteOpen(false);
      return;
    }
    const items = [...selectedEntries];
    setErr(null);
    setDeleteOpen(false);
    try {
      for (const item of items) {
        if (item.kind === "dir") {
          await invoke("control_api_storage_delete_folder", { path: item.path, recursive: deleteRecursive });
        } else {
          await invoke("control_api_storage_delete_file", { path: item.path });
        }
      }
      setSelected(null);
      setSelectedPaths(new Set());
      setLastClickedPath(null);
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const openMove = () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    if (selectedCount === 0) return;
    setMoveTargetDir(path);
    setMoveOpen(true);
  };

  const submitMove = async () => {
    if (selectedEntries.length === 0) {
      setMoveOpen(false);
      return;
    }
    const dir = moveTargetDir.trim().replace(/^\/+|\/+$/g, "");
    setErr(null);
    try {
      for (const item of selectedEntries) {
        const to = dir ? `${dir}/${item.name}` : item.name;
        if (to !== item.path) {
          await invoke("control_api_storage_rename", { from: item.path, to });
        }
      }
      setMoveOpen(false);
      setSelected(null);
      setSelectedPaths(new Set());
      setLastClickedPath(null);
      setPath(dir);
      await refresh(dir);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const submitUploadPlan = async (plan: UploadPlanItem[]) => {
    if (plan.length === 0) return;
    setUploadBusy(true);
    setErr(null);
    try {
      for (const item of plan) {
        await invoke("control_api_storage_upload_file", { remotePath: item.remote, localFile: item.local });
      }
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setUploadBusy(false);
    }
  };

  const onUpload = async () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    const locals = await invoke<string[] | null>("pick_files_for_storage_upload");
    if (!locals || locals.length === 0) return;
    const existing = new Set(list?.entries.map((e) => e.path) ?? []);
    const plan: UploadPlanItem[] = locals.map((local) => {
      const name = local.split(/[/\\]/).pop() ?? "file.bin";
      const remote = path ? `${path}/${name}` : name;
      return { local, remote, name, exists: existing.has(remote) };
    });
    const duplicateRemotes = new Set<string>();
    const seen = new Set<string>();
    for (const item of plan) {
      if (seen.has(item.remote)) duplicateRemotes.add(item.remote);
      seen.add(item.remote);
    }
    if (duplicateRemotes.size > 0) {
      setErr(
        tr(
          `Выбраны файлы с одинаковыми именами: ${Array.from(duplicateRemotes).join(", ")}`,
          `Selected files contain duplicate names: ${Array.from(duplicateRemotes).join(", ")}`,
        ),
      );
      return;
    }
    const conflicts = plan.filter((item) => item.exists);
    if (conflicts.length > 0) {
      setPendingUploadPlan(plan);
      setUploadConflictOpen(true);
      return;
    }
    await submitUploadPlan(plan);
  };

  const onDownload = async () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    if (!selected || selected.kind !== "file") return;
    const save = await invoke<string | null>("pick_save_path_for_storage_download", {
      suggested: selected.name,
    });
    if (!save) return;
    setErr(null);
    try {
      await invoke("control_api_storage_download_file", { remotePath: selected.path, localPath: save });
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const runExtract = async (conflictMode: "abort" | "overwrite" | "delete_and_overwrite") => {
    if (!selected || selected.kind !== "file" || !isStorageArchiveName(selected.name)) {
      return;
    }
    setExtractBusy(true);
    setErr(null);
    setExtractOkHint(null);
    setExtractWarnings(null);
    try {
      const raw = await invoke<string>("control_api_storage_extract_json", {
        archivePath: selected.path,
        targetDir: null as string | null,
        conflictMode: conflictMode,
      });
      setExtractConflict(null);
      const j = JSON.parse(raw) as { extracted_files?: number; created_dirs?: number; warnings?: string[] };
      const files = j.extracted_files ?? 0;
      const dirs = j.created_dirs ?? 0;
      const warns = j.warnings ?? [];
      setExtractOkHint(
        tr(
          `Готово. Файлов: ${files}, папок: ${dirs}.${warns.length > 0 ? ` Предупреждений: ${warns.length} — см. список ниже.` : ""}`,
          `Done. Files: ${files}, folders: ${dirs}.${warns.length > 0 ? ` Warnings: ${warns.length} — see below.` : ""}`,
        ),
      );
      setExtractWarnings(warns.length > 0 ? warns : null);
      await refresh();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      const p = parseStorageHttpError(msg);
      if (p?.status === 409 && p.body?.error?.code === "extract_conflict") {
        setExtractConflict({ path: p.body.error.conflict_path ?? selected.path });
        return;
      }
      setErr(msg);
    } finally {
      setExtractBusy(false);
    }
  };

  const onExtractClick = () => {
    if (!tauri) {
      setErr(t("storage.tauriOnly"));
      return;
    }
    if (!selected || selected.kind !== "file" || !isStorageArchiveName(selected.name)) {
      setErr(t("storage.extractUnsupported"));
      return;
    }
    void runExtract("abort");
  };

  const showExtract = selected?.kind === "file" && isStorageArchiveName(selected.name);

  const openBindModal = (mountPoint: string) => {
    setBindModalSource(mountPoint);
    const parts = mountPoint.split("/").filter(Boolean);
    setBindModalVolume(parts[parts.length - 1] ?? "volume");
    setBindModalOpen(true);
  };

  const submitBind = async () => {
    if (!bindModalSource) return;
    const v = bindModalVolume.trim();
    if (!v) {
      setErr(t("storage.bindVolumeRequired"));
      return;
    }
    setBindBusy(true);
    setErr(null);
    try {
      await invoke<string>("control_api_storage_bind_json", {
        sourcePath: bindModalSource,
        volumeName: v,
      });
      setBindModalOpen(false);
      setBindModalSource(null);
      setBindModalVolume("");
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBindBusy(false);
    }
  };

  const onUnbindVolume = async (volume: string) => {
    setBindBusy(true);
    setErr(null);
    try {
      await invoke<string>("control_api_storage_unbind_json", { volumeName: volume });
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBindBusy(false);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      {!tauri ? (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-100">
          {t("storage.tauriOnly")}
        </div>
      ) : null}

      <ModalDialog
        open={newFolderOpen}
        onClose={() => setNewFolderOpen(false)}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        panelClassName="w-full max-w-md"
        aria-labelledby="storage-newfolder-title"
      >
        <form
          className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl"
          onSubmit={(e) => {
            e.preventDefault();
            void submitNewFolder();
          }}
        >
          <h3 id="storage-newfolder-title" className="text-sm font-semibold text-slate-100">
            {t("storage.modalNewFolderTitle")}
          </h3>
          <label className="mt-3 block text-xs text-slate-400" htmlFor="storage-newfolder-input">
            {t("storage.modalNewFolderLabel")}
          </label>
          <input
            id="storage-newfolder-input"
            className="mt-1 w-full rounded border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-xs text-slate-200 outline-none focus:border-amber-500/50"
            value={newFolderName}
            onChange={(e) => setNewFolderName(e.target.value)}
            autoFocus
            autoComplete="off"
          />
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => setNewFolderOpen(false)}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="submit"
              className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60"
            >
              {t("storage.modalSave")}
            </button>
          </div>
        </form>
      </ModalDialog>

      <ModalDialog
        open={renameOpen}
        onClose={() => setRenameOpen(false)}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        panelClassName="w-full max-w-md"
        aria-labelledby="storage-rename-title"
      >
        <form
          className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl"
          onSubmit={(e) => {
            e.preventDefault();
            void submitRename();
          }}
        >
          <h3 id="storage-rename-title" className="text-sm font-semibold text-slate-100">
            {t("storage.modalRenameTitle")}
          </h3>
          <label className="mt-3 block text-xs text-slate-400" htmlFor="storage-rename-input">
            {t("storage.modalRenameLabel")}
          </label>
          <input
            id="storage-rename-input"
            className="mt-1 w-full rounded border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-xs text-slate-200 outline-none focus:border-amber-500/50"
            value={renameValue}
            onChange={(e) => setRenameValue(e.target.value)}
            autoFocus
            autoComplete="off"
          />
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => setRenameOpen(false)}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="submit"
              className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60"
            >
              {t("storage.modalSave")}
            </button>
          </div>
        </form>
      </ModalDialog>

      <ModalDialog
        open={deleteOpen}
        onClose={() => setDeleteOpen(false)}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        role="alertdialog"
        panelClassName="w-full max-w-md"
        aria-labelledby="storage-delete-title"
      >
        <div className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl">
          <h3 id="storage-delete-title" className="text-sm font-semibold text-slate-100">
            {t("storage.modalDeleteTitle")}
          </h3>
          {selectedEntries.length > 0 ? (
            <p className="mt-2 break-all font-mono text-xs text-amber-200/80">
              {selectedEntries.length > 1
                ? tr(`Выбрано элементов: ${selectedEntries.length}`, `Selected items: ${selectedEntries.length}`)
                : selectedEntries[0]!.path}
            </p>
          ) : null}
          <p className="mt-2 text-xs text-slate-400">{t("storage.modalDeleteBody")}</p>
          {hasSelectedDirs ? (
            <label className="mt-3 flex cursor-pointer items-center gap-2 text-xs text-slate-300">
              <input
                type="checkbox"
                className="rounded border border-white/20"
                checked={deleteRecursive}
                onChange={(e) => setDeleteRecursive(e.target.checked)}
              />
              {t("storage.modalDeleteRecursive")}
            </label>
          ) : null}
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => setDeleteOpen(false)}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="button"
              onClick={() => void submitDelete()}
              className="rounded-lg border border-rose-800/40 bg-rose-950/35 px-3 py-1.5 text-xs text-rose-100 hover:bg-rose-950/55"
            >
              {t("storage.delete")}
            </button>
          </div>
        </div>
      </ModalDialog>

      <ModalDialog
        open={moveOpen}
        onClose={() => setMoveOpen(false)}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        panelClassName="w-full max-w-md"
        aria-labelledby="storage-move-title"
      >
        <form
          className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl"
          onSubmit={(e) => {
            e.preventDefault();
            void submitMove();
          }}
        >
          <h3 id="storage-move-title" className="text-sm font-semibold text-slate-100">
            {t("storage.modalMoveTitle")}
          </h3>
          <p className="mt-2 break-all font-mono text-xs text-amber-200/80">
            {selectedCount > 1
              ? tr(`Выбрано элементов: ${selectedCount}`, `Selected items: ${selectedCount}`)
              : selectedEntries[0]?.path ?? ""}
          </p>
          <label className="mt-3 block text-xs text-slate-400" htmlFor="storage-move-input">
            {t("storage.modalMoveLabel")}
          </label>
          <input
            id="storage-move-input"
            className="mt-1 w-full rounded border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-xs text-slate-200 outline-none focus:border-amber-500/50"
            value={moveTargetDir}
            onChange={(e) => setMoveTargetDir(e.target.value)}
            autoFocus
            autoComplete="off"
          />
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => setMoveOpen(false)}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="submit"
              className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60"
            >
              {t("storage.modalMoveSubmit")}
            </button>
          </div>
        </form>
      </ModalDialog>

      <ModalDialog
        open={bindModalOpen}
        onClose={() => {
          setBindModalOpen(false);
          setBindModalSource(null);
          setBindModalVolume("");
        }}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        panelClassName="w-full max-w-md"
        aria-labelledby="storage-bind-modal-title"
      >
        <form
          className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl"
          onSubmit={(e) => {
            e.preventDefault();
            void submitBind();
          }}
        >
          <h3 id="storage-bind-modal-title" className="text-sm font-semibold text-slate-100">
            {t("storage.bindModalTitle")}
          </h3>
          <p className="mt-2 text-xs text-slate-400">
            {t("storage.bindModalMount")}{" "}
            <code className="break-all font-mono text-amber-200/80">{bindModalSource ?? ""}</code>
          </p>
          <label className="mt-3 block text-xs text-slate-400" htmlFor="storage-bind-volume-input">
            {t("storage.bindModalVolumeLabel")}
          </label>
          <input
            id="storage-bind-volume-input"
            className="mt-1 w-full rounded border border-white/10 bg-black/30 px-2 py-1.5 font-mono text-xs text-slate-200 outline-none focus:border-amber-500/50"
            value={bindModalVolume}
            onChange={(e) => setBindModalVolume(e.target.value)}
            autoFocus
            autoComplete="off"
          />
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                setBindModalOpen(false);
                setBindModalSource(null);
                setBindModalVolume("");
              }}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.modalCancel")}
            </button>
            <button
              type="submit"
              disabled={bindBusy}
              className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60 disabled:opacity-50"
            >
              {t("storage.bindAttach")}
            </button>
          </div>
        </form>
      </ModalDialog>

      <ModalDialog
        open={uploadConflictOpen}
        onClose={() => setUploadConflictOpen(false)}
        zClassName="z-modalConfirm"
        closeOnBackdrop
        panelClassName="w-full max-w-lg"
        aria-labelledby="storage-upload-conflict-title"
      >
        <div className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl">
          <h3 id="storage-upload-conflict-title" className="text-sm font-semibold text-slate-100">
            {t("storage.uploadConflictTitle")}
          </h3>
          <p className="mt-2 text-xs text-slate-400">{t("storage.uploadConflictBody")}</p>
          <ul className="mt-2 max-h-40 space-y-1 overflow-auto rounded border border-white/10 bg-black/20 px-2 py-1.5">
            {pendingUploadPlan
              .filter((item) => item.exists)
              .map((item) => (
                <li key={item.remote} className="break-all font-mono text-[11px] text-amber-200/80">
                  {item.remote}
                </li>
              ))}
          </ul>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                setUploadConflictOpen(false);
                setPendingUploadPlan([]);
              }}
              className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
            >
              {t("storage.uploadConflictCancel")}
            </button>
            <button
              type="button"
              onClick={() => {
                const plan = [...pendingUploadPlan];
                setUploadConflictOpen(false);
                setPendingUploadPlan([]);
                void submitUploadPlan(plan);
              }}
              className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60"
            >
              {t("storage.uploadConflictOverwrite")}
            </button>
          </div>
        </div>
      </ModalDialog>

      {tauri ? (
        <div className="inline-flex flex-wrap gap-1 rounded-lg border border-border-subtle bg-panel/80 p-1 shadow-card">
          <button
            type="button"
            onClick={() => setStorageView("files")}
            className={`inline-flex items-center gap-1 rounded px-3 py-1.5 text-xs font-medium transition-colors ${
              storageView === "files" ? "bg-red-950/40 text-amber-100" : "text-slate-400 hover:bg-white/5"
            }`}
          >
            <Folder className="h-3.5 w-3.5" />
            {t("storage.subtab.files")}
          </button>
          <button
            type="button"
            onClick={() => setStorageView("databases")}
            className={`inline-flex items-center gap-1 rounded px-3 py-1.5 text-xs font-medium transition-colors ${
              storageView === "databases" ? "bg-red-950/40 text-amber-100" : "text-slate-400 hover:bg-white/5"
            }`}
          >
            <Database className="h-3.5 w-3.5" />
            {t("storage.subtab.databases")}
          </button>
        </div>
      ) : null}

      {storageView === "databases" && tauri ? (
        <div className="flex min-h-0 flex-1 flex-col gap-3 md:min-h-[min(80vh,680px)]">
          {hasHostDatabases ? (
            <div
              className="inline-flex flex-wrap gap-1 rounded-xl border border-amber-900/20 bg-amber-950/10 p-1 shadow-[inset_0_0_0_1px_rgba(251,191,36,0.06)]"
              role="tablist"
              aria-label={t("storage.databasesModeAria")}
            >
              <button
                type="button"
                role="tab"
                aria-selected={storageDbMode === "host"}
                onClick={() => setStorageDbMode("host")}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition-colors ${
                  storageDbMode === "host"
                    ? "bg-amber-900/30 text-amber-100 shadow-sm"
                    : "text-slate-400 hover:bg-white/5 hover:text-slate-200"
                }`}
              >
                <Server className="h-3.5 w-3.5 text-amber-500/80" />
                {t("storage.dbModeHost")}
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={storageDbMode === "direct"}
                onClick={() => setStorageDbMode("direct")}
                className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition-colors ${
                  storageDbMode === "direct"
                    ? "bg-amber-900/30 text-amber-100 shadow-sm"
                    : "text-slate-400 hover:bg-white/5 hover:text-slate-200"
                }`}
              >
                <Database className="h-3.5 w-3.5 text-amber-500/80" />
                {t("storage.dbModeDirect")}
              </button>
            </div>
          ) : (
            <p className="text-[11px] leading-relaxed text-slate-500">
              {t("storage.databasesDirectOnlyHint")}
            </p>
          )}

          {hasHostDatabases && storageDbMode === "host" && hostDbList ? (
            <DatabasesPanel
              instances={hostDbList.instances}
              onRefresh={reloadHostDatabases}
              loadingList={hostDbLoading}
            />
          ) : null}
          {(!hasHostDatabases || storageDbMode === "direct") ? <DbExplorerPanel embedInStorage /> : null}
        </div>
      ) : null}

      {extractConflict ? (
        <ModalDialog
          open
          onClose={() => setExtractConflict(null)}
          zClassName="z-modalConfirm"
          closeOnBackdrop={false}
          panelClassName="w-full max-w-md"
          aria-labelledby="storage-extract-conflict-title"
        >
          <div className="rounded-2xl border border-white/10 bg-[#0a0908] p-4 shadow-xl">
            <h3 id="storage-extract-conflict-title" className="text-sm font-semibold text-slate-100">
              {t("storage.extractConflictTitle")}
            </h3>
            <p className="mt-2 text-xs text-slate-400">
              {t("storage.extractConflictBody")}{" "}
              <code className="break-all text-amber-200/80">{extractConflict.path}</code>
            </p>
            <div className="mt-4 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                onClick={() => setExtractConflict(null)}
                className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 hover:bg-white/10"
              >
                {t("storage.extractCancel")}
              </button>
              <button
                type="button"
                disabled={extractBusy}
                onClick={() => {
                  setExtractConflict(null);
                  void runExtract("overwrite");
                }}
                className="rounded-lg border border-amber-800/40 bg-amber-950/40 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-950/60 disabled:opacity-50"
              >
                {t("storage.extractOverwrite")}
              </button>
              <button
                type="button"
                disabled={extractBusy}
                onClick={() => {
                  setExtractConflict(null);
                  void runExtract("delete_and_overwrite");
                }}
                className="rounded-lg border border-rose-800/40 bg-rose-950/35 px-3 py-1.5 text-xs text-rose-100 hover:bg-rose-950/55 disabled:opacity-50"
              >
                {t("storage.extractDeleteOverwrite")}
              </button>
            </div>
          </div>
        </ModalDialog>
      ) : null}

      {storageView === "files" ? (
      <>
      <div className="flex flex-col gap-2 rounded-lg border border-border-subtle bg-panel p-3 shadow-card sm:flex-row sm:items-center sm:justify-between">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold text-slate-200">{t("storage.title")}</h2>
          {usage ? (
            <p className="mt-1 text-xs text-slate-500">
              {tr("Использовано", "Used")}{" "}
              <span className="font-mono text-slate-300">{formatBytes(usage.used_bytes)}</span>
              {usage.max_bytes > 0 ? (
                <>
                  {" "}
                  / {formatBytes(usage.max_bytes)}
                  {usage.free_bytes != null && usage.free_bytes !== undefined ? (
                    <span className="text-slate-500"> ({tr("свободно", "free")} {formatBytes(usage.free_bytes)})</span>
                  ) : null}
                </>
              ) : (
                <span className="text-slate-500"> ({tr("без лимита", "unlimited")})</span>
              )}
            </p>
          ) : (
            <p className="text-xs text-slate-500">—</p>
          )}
          {usage && usage.max_bytes > 0 && usage.used_percent != null && usage.used_percent !== undefined ? (
            <div className="mt-2 h-1.5 w-full max-w-md overflow-hidden rounded bg-black/50">
              <div
                className="h-full bg-red-600/80"
                style={{ width: `${Math.min(100, Math.max(0, usage.used_percent))}%` }}
              />
            </div>
          ) : null}
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-black/20 px-2 py-1.5 text-xs text-slate-200 hover:bg-black/30 disabled:opacity-50"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          {t("storage.refresh")}
        </button>
      </div>

      {tauri ? (
        <div className="rounded-lg border border-border-subtle bg-panel p-3 shadow-card">
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="min-w-0">
              <h3 className="inline-flex items-center gap-1.5 text-xs font-semibold text-slate-200">
                <HardDrive className="h-3.5 w-3.5 shrink-0 text-amber-500/70" />
                {t("storage.bindVolumesTitle")}
              </h3>
              <p className="mt-1 text-[11px] leading-relaxed text-slate-500">{t("storage.bindVolumesHint")}</p>
            </div>
            <button
              type="button"
              onClick={() => void refresh()}
              disabled={loading}
              className="shrink-0 rounded border border-border-subtle bg-black/20 px-2 py-1 text-[11px] text-slate-300 hover:bg-black/30 disabled:opacity-50"
            >
              {t("storage.bindRefresh")}
            </button>
          </div>

          <div className="mt-3 grid gap-4 md:grid-cols-2">
            <div className="min-w-0">
              <h4 className="text-[11px] font-medium uppercase tracking-wide text-slate-400">
                {t("storage.bindActiveTitle")}
              </h4>
              {!bindView || bindView.active_binds.length === 0 ? (
                <p className="mt-1 text-[11px] text-slate-500">{t("storage.bindEmptyActive")}</p>
              ) : (
                <div className="mt-1 max-h-40 overflow-auto rounded border border-white/10">
                  <table className="w-full border-collapse text-left text-[11px]">
                    <thead className="sticky top-0 bg-[#0f0e0d] text-slate-400">
                      <tr>
                        <th className="border-b border-white/10 px-2 py-1 font-normal">
                          {t("storage.bindColVolume")}
                        </th>
                        <th className="border-b border-white/10 px-2 py-1 font-normal">{t("storage.bindColMount")}</th>
                        <th className="border-b border-white/10 px-2 py-1 font-normal" />
                      </tr>
                    </thead>
                    <tbody>
                      {bindView.active_binds.map((b) => (
                        <tr key={b.volume} className="border-b border-white/5 text-slate-200">
                          <td className="px-2 py-1 font-mono text-amber-200/90">{b.volume}</td>
                          <td className="break-all px-2 py-1 font-mono text-slate-400" title={b.source}>
                            {b.source}
                          </td>
                          <td className="px-2 py-1 text-right">
                            <button
                              type="button"
                              disabled={bindBusy}
                              onClick={() => void onUnbindVolume(b.volume)}
                              className="rounded border border-rose-900/40 bg-rose-950/30 px-1.5 py-0.5 text-rose-100/90 hover:bg-rose-950/50 disabled:opacity-40"
                            >
                              {t("storage.bindDetach")}
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>

            <div className="min-w-0">
              <h4 className="text-[11px] font-medium uppercase tracking-wide text-slate-400">
                {t("storage.bindCandidatesTitle")}
              </h4>
              {!bindView || bindView.candidates.length === 0 ? (
                <p className="mt-1 text-[11px] text-slate-500">{t("storage.bindEmptyCandidates")}</p>
              ) : (
                <div className="mt-1 max-h-48 overflow-auto rounded border border-white/10">
                  <table className="w-full border-collapse text-left text-[11px]">
                    <thead className="sticky top-0 bg-[#0f0e0d] text-slate-400">
                      <tr>
                        <th className="border-b border-white/10 px-2 py-1 font-normal">{t("storage.bindColMount")}</th>
                        <th className="border-b border-white/10 px-2 py-1 font-normal">{t("storage.bindColFstype")}</th>
                        <th className="border-b border-white/10 px-2 py-1 font-normal">{t("storage.bindColAvail")}</th>
                        <th className="border-b border-white/10 px-2 py-1 font-normal" />
                      </tr>
                    </thead>
                    <tbody>
                      {bindView.candidates.map((c) => {
                        const bound = isMountBoundAsSource(c.mount_point, bindView.active_binds);
                        return (
                          <tr key={c.mount_point} className="border-b border-white/5 text-slate-200">
                            <td className="break-all px-2 py-1 font-mono text-amber-200/80">{c.mount_point}</td>
                            <td className="px-2 py-1 font-mono text-slate-400">{c.fstype}</td>
                            <td className="px-2 py-1 font-mono text-slate-400">
                              {c.avail_bytes != null && c.avail_bytes !== undefined ? formatBytes(c.avail_bytes) : "—"}
                            </td>
                            <td className="px-2 py-1 text-right">
                              <button
                                type="button"
                                disabled={bindBusy || bound}
                                onClick={() => openBindModal(c.mount_point)}
                                className="rounded border border-amber-900/40 bg-amber-950/30 px-1.5 py-0.5 text-amber-100/90 hover:bg-amber-950/45 disabled:cursor-not-allowed disabled:opacity-35"
                              >
                                {t("storage.bindAttach")}
                              </button>
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          </div>
          {bindBusy ? <p className="mt-2 text-[11px] text-slate-400">{t("storage.bindBusy")}</p> : null}
        </div>
      ) : null}

      {err ? (
        <p className="rounded border border-rose-900/50 bg-rose-950/30 px-3 py-2 text-xs text-rose-200">{err}</p>
      ) : null}
      {extractOkHint ? (
        <p className="rounded border border-emerald-900/50 bg-emerald-950/25 px-3 py-2 text-xs text-emerald-200/90">
          {extractOkHint}
        </p>
      ) : null}
      {extractWarnings && extractWarnings.length > 0 ? (
        <ul className="max-h-40 list-inside list-disc space-y-1 overflow-auto rounded border border-amber-800/50 bg-amber-950/20 px-3 py-2 text-[10px] text-amber-100/90">
          {extractWarnings.map((w, i) => (
            <li key={i} className="break-all font-mono">
              {w}
            </li>
          ))}
        </ul>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => {
            setPath(parentPath());
            setSelected(null);
            setSelectedPaths(new Set());
            setLastClickedPath(null);
          }}
          disabled={!path}
          className="rounded border border-border-subtle bg-black/20 px-2 py-1 text-xs text-slate-200 disabled:opacity-40"
        >
          ..
        </button>
        <code className="max-w-full truncate rounded border border-border-subtle bg-black/30 px-2 py-1 font-mono text-[11px] text-orange-200/80">
          /{path}
        </code>
      </div>

      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          onClick={() => openNewFolder()}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100"
        >
          <FolderPlus className="h-3.5 w-3.5" />
          {t("storage.newFolder")}
        </button>
        <button
          type="button"
          onClick={() => void onUpload()}
          disabled={uploadBusy}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100 disabled:opacity-40"
        >
          <FileUp className="h-3.5 w-3.5" />
          {t("storage.upload")}
        </button>
        <button
          type="button"
          onClick={() => void onDownload()}
          disabled={!selected || !hasExactlyOneSelected || selected.kind !== "file"}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100 disabled:opacity-40"
        >
          <Download className="h-3.5 w-3.5" />
          {t("storage.download")}
        </button>
        <button
          type="button"
          onClick={() => onExtractClick()}
          disabled={!showExtract || extractBusy || !hasExactlyOneSelected}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100 disabled:opacity-40"
        >
          <PackageOpen className="h-3.5 w-3.5" />
          {t("storage.extract")}
        </button>
        <button
          type="button"
          onClick={() => openRename()}
          disabled={!selected || !hasExactlyOneSelected}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100 disabled:opacity-40"
        >
          <Pencil className="h-3.5 w-3.5" />
          {t("storage.rename")}
        </button>
        <button
          type="button"
          onClick={() => openMove()}
          disabled={selectedCount === 0}
          className="inline-flex items-center gap-1 rounded border border-border-subtle bg-red-950/30 px-2 py-1.5 text-xs text-slate-100 disabled:opacity-40"
        >
          <Folder className="h-3.5 w-3.5" />
          {t("storage.move")}
        </button>
        <button
          type="button"
          onClick={() => openDelete()}
          disabled={selectedCount === 0}
          className="inline-flex items-center gap-1 rounded border border-rose-900/40 bg-rose-950/25 px-2 py-1.5 text-xs text-rose-100 disabled:opacity-40"
        >
          <Trash2 className="h-3.5 w-3.5" />
          {t("storage.delete")}
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto rounded border border-border-subtle">
        {loading && !list ? (
          <p className="p-4 text-xs text-slate-500">…</p>
        ) : list && list.entries.length === 0 ? (
          <p className="p-4 text-xs text-slate-500">{t("storage.empty")}</p>
        ) : list ? (
          <table className="w-full min-w-[28rem] border-collapse text-left text-xs">
            <thead>
              <tr className="border-b border-border-subtle bg-black/20 text-slate-500">
                <th className="w-10 px-3 py-2 font-medium">
                  <input
                    type="checkbox"
                    className="rounded border border-white/20"
                    checked={list.entries.length > 0 && selectedCount === list.entries.length}
                    onChange={(ev) => {
                      if (ev.target.checked) {
                        const next = new Set<string>(list.entries.map((entry) => entry.path));
                        setSelectedPaths(next);
                        const last = list.entries[list.entries.length - 1];
                        if (last) {
                          setSelected(last);
                          setLastClickedPath(last.path);
                        }
                      } else {
                        setSelectedPaths(new Set());
                        setSelected(null);
                        setLastClickedPath(null);
                      }
                    }}
                  />
                </th>
                <th className="px-3 py-2 font-medium">{t("storage.colName")}</th>
                <th className="px-3 py-2 font-medium">{t("storage.colKind")}</th>
                <th className="px-3 py-2 font-medium">{t("storage.colSize")}</th>
              </tr>
            </thead>
            <tbody>
              {list.entries.map((e) => (
                <StorageRow
                  key={e.path + e.name}
                  entry={e}
                  selected={selectedPaths.has(e.path)}
                  onRange={selectRange}
                  onToggle={toggleSelection}
                  onSingle={setSingleSelection}
                  onOpen={enter}
                />
              ))}
            </tbody>
          </table>
        ) : null}
      </div>
      <p className="text-[10px] text-slate-500">{t("storage.hint")}</p>
      </>
      ) : null}
    </div>
  );
}
