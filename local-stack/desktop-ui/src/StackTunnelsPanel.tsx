import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Edge, Node, OnBeforeDelete } from "@xyflow/react";
import { Loader2, PencilLine, Plus, Rocket, RotateCcw, Save, Shield, Trash2, X } from "lucide-react";
import React, { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { useIntervalWhenVisible } from "./hooks/useIntervalWhenVisible";
import { useI18n } from "./i18n";
import { ModalDialog } from "./ui/ModalDialog";
import {
  BOOKMARK_SERVER_NODE_TYPE,
  LOCAL_PC_NODE_ID,
  LOCAL_PC_NODE_TYPE,
  blankEdgeBetween,
  canvasToTunnelProfiles,
  defaultBookmarkNode,
  defaultLocalPcNode,
  emptyCanvasPreset,
  guessStackTunGrpcFromGrpcUrl,
  guessStackTunHttpFromGrpcUrl,
  normalizeStackTunUrl,
  stringifyProfilesPretty,
  tunnelProfilesToCanvas,
  validateStackTunControlHttpUrl,
  validateStackTunRelayGrpcUrl,
  type ServerBookmarkBrief,
  type TunnelBookmarkNodeData,
  type TunnelCanvasConfig,
  type TunnelEdgeData,
  type TunnelLinkMode,
  type TunnelLocalPcNodeData,
  type TunnelWireMode,
} from "./stackTunnels";

const StackTunnelsFlowEditor = React.lazy(() =>
  import("./StackTunnelsFlowEditor").then((m) => ({ default: m.StackTunnelsFlowEditor })),
);

/** Where to call POST authorize_peer: preset header, else infer :9380 from edge relay or target bookmark gRPC. */
function resolveTargetStackTunControlBase(
  presetHttpBase: string,
  edge: Edge<TunnelEdgeData>,
  nodes: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[],
): string {
  const fromPreset = normalizeStackTunUrl(presetHttpBase);
  if (fromPreset) return fromPreset;
  const relay = (edge.data?.remoteGrpcEndpoint ?? "").trim();
  if (relay) {
    const inferred = normalizeStackTunUrl(guessStackTunHttpFromGrpcUrl(relay));
    if (validateStackTunControlHttpUrl(inferred) === null) return inferred;
  }
  const tn = nodes.find((n) => n.id === edge.target);
  if (tn?.type === BOOKMARK_SERVER_NODE_TYPE) {
    const grpcUrl = ((tn.data as TunnelBookmarkNodeData).grpcUrl ?? "").trim();
    if (grpcUrl) {
      const inferred = normalizeStackTunUrl(guessStackTunHttpFromGrpcUrl(grpcUrl));
      if (validateStackTunControlHttpUrl(inferred) === null) return inferred;
    }
  }
  return "";
}

/** Ordered URLs for GET identity public key on the edge source node (HTTP control :9380, not dashboard :80). */
function collectSourceIdentityHttpBases(
  edge: Edge<TunnelEdgeData>,
  nodes: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[],
  presetHttpBase: string,
): string[] {
  const out: string[] = [];
  const add = (raw: string) => {
    const t = normalizeStackTunUrl(raw);
    if (t && !out.includes(t)) out.push(t);
  };
  const fromField =
    (edge.data?.sourceHttpBase ?? "").trim() ||
    (edge.source === LOCAL_PC_NODE_ID ? presetHttpBase.trim() : "");
  add(fromField);
  const sn = nodes.find((n) => n.id === edge.source);
  if (sn?.type === BOOKMARK_SERVER_NODE_TYPE) {
    const grpcUrl = ((sn.data as TunnelBookmarkNodeData).grpcUrl ?? "").trim();
    if (grpcUrl) add(guessStackTunHttpFromGrpcUrl(grpcUrl));
  }
  return out;
}

const LS_BASE_LEGACY = "pirate.stackTunHttpBase";
const LS_TOKEN_LEGACY = "pirate.stackTunBearer";
const LS_CANVAS_PRESETS = "pirate.stackTun.canvasPresets";

type PresetsBlobV1 = {
  version: 1;
  activeConfigId: string;
  configs: TunnelCanvasConfig[];
};

function newConfigId(): string {
  const u =
    typeof globalThis.crypto !== "undefined" && "randomUUID" in globalThis.crypto
      ? globalThis.crypto.randomUUID()
      : `${Date.now()}_${Math.random().toString(36).slice(2)}`;
  return `cfg_${u}`;
}

function loadLegacyBaseBearer(): Pick<TunnelCanvasConfig, "stackTunHttpBase" | "stackTunBearer"> {
  try {
    return {
      stackTunHttpBase: window.localStorage.getItem(LS_BASE_LEGACY) ?? "",
      stackTunBearer: window.localStorage.getItem(LS_TOKEN_LEGACY) ?? "",
    };
  } catch {
    return { stackTunHttpBase: "", stackTunBearer: "" };
  }
}

function persistLegacyBaseBearer(base: string, bearer: string) {
  try {
    window.localStorage.setItem(LS_BASE_LEGACY, base.trim());
    window.localStorage.setItem(LS_TOKEN_LEGACY, bearer.trim());
  } catch {
    /* ignore */
  }
}

function repairCanvasConfig(raw: unknown): TunnelCanvasConfig | null {
  const c = raw as Partial<TunnelCanvasConfig> | null;
  if (!c || typeof c.id !== "string" || !c.id.trim()) return null;
  const base = emptyCanvasPreset();
  const nodes = Array.isArray(c.rfNodes) && c.rfNodes.length > 0 ? c.rfNodes : base.rfNodes;
  const eds = Array.isArray(c.rfEdges) ? c.rfEdges : base.rfEdges;
  const edgesRepaired = (eds as Edge<TunnelEdgeData>[]).map((e) => {
    if (!e.data) return e;
    const d = e.data as TunnelEdgeData;
    return {
      ...e,
      data: {
        ...d,
        tunnelMode: d.tunnelMode ?? ("tcpRelay" as TunnelWireMode),
        linkMode: (d.linkMode ?? "publicAuth") as TunnelLinkMode,
        sourceHttpBase: d.sourceHttpBase ?? "",
      },
    };
  });
  return {
    id: c.id.trim(),
    name: typeof c.name === "string" && c.name.trim() ? c.name.trim() : "default",
    stackTunHttpBase: typeof c.stackTunHttpBase === "string" ? c.stackTunHttpBase : "",
    stackTunBearer: typeof c.stackTunBearer === "string" ? c.stackTunBearer : "",
    rfNodes: nodes as Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[],
    rfEdges: edgesRepaired,
  };
}

function defaultPresetsFromScratch(): PresetsBlobV1 {
  const leg =
    typeof window !== "undefined"
      ? loadLegacyBaseBearer()
      : { stackTunHttpBase: "", stackTunBearer: "" };
  const id = newConfigId();
  const preset: TunnelCanvasConfig = {
    id,
    name: "default",
    stackTunHttpBase: leg.stackTunHttpBase,
    stackTunBearer: leg.stackTunBearer,
    ...emptyCanvasPreset(),
  };
  return { version: 1, activeConfigId: id, configs: [preset] };
}

function loadPresetsBlob(): PresetsBlobV1 {
  try {
    const raw = window.localStorage.getItem(LS_CANVAS_PRESETS);
    if (!raw) throw new Error("empty");
    const j = JSON.parse(raw) as {
      configs?: unknown[];
      activeConfigId?: string;
    };
    if (!Array.isArray(j.configs) || j.configs.length === 0) throw new Error("bad shape");
    const configs = j.configs.map(repairCanvasConfig).filter((c): c is TunnelCanvasConfig => c !== null);
    if (configs.length === 0) throw new Error("no valid configs");
    let activeConfigId = typeof j.activeConfigId === "string" ? j.activeConfigId.trim() : "";
    if (!configs.some((c) => c.id === activeConfigId)) activeConfigId = configs[0].id;
    return { version: 1, activeConfigId, configs };
  } catch {
    return defaultPresetsFromScratch();
  }
}

function savePresetsBlob(blob: PresetsBlobV1) {
  window.localStorage.setItem(LS_CANVAS_PRESETS, JSON.stringify(blob));
}

type StackStats = {
  listenerAccepts?: number;
  connectorPulls?: number;
  relayCompleted?: number;
  relayErrors?: number;
  requestBusReceived?: number;
  requestBusBlocked?: number;
  requestBusCompleted?: number;
  requestBusErrors?: number;
};

type IdentityPublicKeyResponse = {
  publicKeyB64?: string;
};

function parseStatsJson(raw: string): StackStats | null {
  try {
    return JSON.parse(raw) as StackStats;
  } catch {
    return null;
  }
}

function parsePublicKeyJson(raw: string): string {
  const parsed = JSON.parse(raw) as IdentityPublicKeyResponse;
  const key = parsed.publicKeyB64?.trim();
  if (!key) throw new Error("publicKeyB64 missing in stack-tun response");
  return key;
}

function appendTokenList(raw: string, token: string): string {
  const parts = raw
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);
  if (!parts.includes(token)) parts.push(token);
  return parts.join("\n");
}

export function StackTunnelsPanel() {
  const { language, t } = useI18n();
  const tr = useCallback((ru: string, en: string) => (language === "ru" ? ru : en), [language]);

  const [presets, setPresets] = useState<PresetsBlobV1>(() =>
    typeof window !== "undefined" ? loadPresetsBlob() : defaultPresetsFromScratch(),
  );

  const [bookmarks, setBookmarks] = useState<ServerBookmarkBrief[]>([]);
  const [busy, setBusy] = useState(false);
  const [statsRaw, setStatsRaw] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editorBusTab, setEditorBusTab] = useState<"schema" | "requests" | "rules">("schema");
  const [reqLogRaw, setReqLogRaw] = useState<string>("[]");
  const [routesEditor, setRoutesEditor] = useState<string>('{\n  "routes": []\n}');
  const [peersPretty, setPeersPretty] = useState<string>("");
  const [reqLive, setReqLive] = useState(false);
  const [reqFilter, setReqFilter] = useState({
    limit: "80",
    host: "",
    pathPart: "",
    method: "",
    profileId: "",
    traceId: "",
    requestId: "",
    errorsOnly: false,
    blockedOnly: false,
  });

  /** Selection from React Flow (last interaction). */
  const [picked, setPicked] = useState<{
    kind: "edge" | "node" | "none";
    edge?: Edge<TunnelEdgeData>;
    node?: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>;
  }>({ kind: "none" });

  const lastHydratedPresetIdRef = useRef<string | null>(null);

  const activeConfig = useMemo((): TunnelCanvasConfig => {
    const c =
      presets.configs.find((x) => x.id === presets.activeConfigId) ?? presets.configs[0];
    if (c) return c;
    return {
      id: "_",
      name: "default",
      stackTunHttpBase: "",
      stackTunBearer: "",
      ...emptyCanvasPreset(),
    };
  }, [presets.activeConfigId, presets.configs]);

  const [nodes, setNodes] = useState<Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[]>([]);
  const [edges, setEdges] = useState<Edge<TunnelEdgeData>[]>([]);

  const mergeActivePreset = useCallback(
    (
      updater: Partial<TunnelCanvasConfig> | ((prev: TunnelCanvasConfig) => TunnelCanvasConfig),
    ): void => {
      setPresets((prevBlob) => {
        const cid = prevBlob.activeConfigId;
        let nextCfgs = prevBlob.configs.map((c) => {
          if (c.id !== cid) return c;
          const base = JSON.parse(JSON.stringify(c)) as TunnelCanvasConfig;
          const next =
            typeof updater === "function"
              ? (updater as (p: TunnelCanvasConfig) => TunnelCanvasConfig)(base)
              : { ...base, ...updater };
          return next;
        });
        savePresetsBlob({ ...prevBlob, configs: nextCfgs });
        return { ...prevBlob, configs: nextCfgs };
      });
    },
    [],
  );

  /** When switching named preset — load its saved graph snapshot (don't clobber edits on unrelated preset updates). */
  useEffect(() => {
    const aid = presets.activeConfigId;
    if (!aid) return;
    if (lastHydratedPresetIdRef.current === aid) return;
    lastHydratedPresetIdRef.current = aid;
    const c = presets.configs.find((x) => x.id === aid);
    if (!c) return;
    setNodes(
      JSON.parse(JSON.stringify(c.rfNodes)) as Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[],
    );
    setEdges(JSON.parse(JSON.stringify(c.rfEdges)) as Edge<TunnelEdgeData>[]);
    setPicked({ kind: "none" });
  }, [presets.activeConfigId, presets.configs, setEdges, setNodes]);

  /** Load bookmarks via Tauri. */
  useEffect(() => {
    if (!isTauri()) return;
    void invoke<Record<string, string>[]>("list_server_bookmarks")
      .then((list) =>
        Array.isArray(list)
          ? list.map((raw) => ({
              id: String(raw?.id ?? ""),
              label: String(raw?.label ?? ""),
              url: String(raw?.url ?? ""),
            })).filter((b) => b.id && b.url)
          : [],
      )
      .then(setBookmarks)
      .catch(() => setBookmarks([]));
  }, []);

  const snapshotCanvasIntoPreset = useCallback(() => {
    mergeActivePreset((prev) => {
      persistLegacyBaseBearer(prev.stackTunHttpBase, prev.stackTunBearer);
      return {
        ...prev,
        rfNodes: JSON.parse(JSON.stringify(nodes)) as Node<
          TunnelLocalPcNodeData | TunnelBookmarkNodeData
        >[],
        rfEdges: JSON.parse(JSON.stringify(edges)) as Edge<TunnelEdgeData>[],
      };
    });
    toast.success(t("tunnels.canvas.savedPreset"));
  }, [edges, mergeActivePreset, nodes, t]);

  const onPersistHeaderFields = () => {
    mergeActivePreset((prev) => ({ ...prev }));
    persistLegacyBaseBearer(activeConfig.stackTunHttpBase, activeConfig.stackTunBearer);
  };

  const baseUrl = activeConfig.stackTunHttpBase.trim();
  const bearerTok = activeConfig.stackTunBearer.trim();

  const onHealth = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    onPersistHeaderFields();
    setBusy(true);
    try {
      const s = await invoke<string>("stack_tun_health", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      toast.success(s.slice(0, 120));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onFetchServer = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    onPersistHeaderFields();
    setBusy(true);
    try {
      const raw = await invoke<string>("stack_tun_get_config", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      const imported = tunnelProfilesToCanvas(raw, bookmarks);
      setNodes(imported.rfNodes as Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[]);
      setEdges(imported.rfEdges);
      toast.success(t("tunnels.canvas.fetchedRebuild"));
      /* Auto-save fetched layout into preset for convenience */
      persistLegacyBaseBearer(baseUrl, bearerTok);
      mergeActivePreset((prev) => ({
        ...prev,
        stackTunHttpBase: prev.stackTunHttpBase.trim(),
        stackTunBearer: prev.stackTunBearer.trim(),
        rfNodes: JSON.parse(JSON.stringify(imported.rfNodes)) as Node<
          TunnelLocalPcNodeData | TunnelBookmarkNodeData
        >[],
        rfEdges: JSON.parse(JSON.stringify(imported.rfEdges)),
      }));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const validateBeforePush = (): string | null => {
    const errs: string[] = [];

    const headerCtrl = validateStackTunControlHttpUrl(activeConfig.stackTunHttpBase.trim());
    if (headerCtrl === "empty") errs.push(t("tunnels.urls.controlEmpty"));
    if (headerCtrl === "grpc_port_in_http_field") errs.push(t("tunnels.urls.grpcPortInControl"));
    if (headerCtrl === "bad_scheme") errs.push(t("tunnels.urls.controlBad"));

    const serverNodeIds = new Set(nodes.filter((n) => n.type === BOOKMARK_SERVER_NODE_TYPE).map((n) => n.id));
    for (const e of edges) {
      if (!serverNodeIds.has(e.target) || e.source === e.target) continue;
      const d = e.data;
      if (!d) {
        errs.push(t("tunnels.canvas.errEdgeData"));
        continue;
      }

      const mode = d.tunnelMode ?? "tcpRelay";

      const gr = validateStackTunRelayGrpcUrl(d.remoteGrpcEndpoint.trim());
      if (gr === "empty") errs.push(`${e.id}: ${t("tunnels.canvas.remoteGrpcRequired")}`);
      if (gr === "http_port_in_grpc_field") errs.push(`${e.id}: ${t("tunnels.urls.httpPortInRelay")}`);
      if (gr === "bad_scheme") errs.push(`${e.id}: ${t("tunnels.urls.relayBad")}`);

      if (mode === "tcpRelay" && !(d.listenAddr || "").trim()) errs.push(`${e.id}: ${t("tunnels.listenAddr")}`);
      if (mode !== "tcpRelay") {
        // requestBus listeners may omit TCP bind; placeholder allows empty canvas field.
      }
      if ((d.linkMode ?? "publicAuth") === "publicAuth") {
        if (!(d.connectorAllowPubkeyB64 || "").trim())
          errs.push(`${e.id}: ${t("tunnels.canvas.allowPubkeys")}`);
      }
      if ((d.linkMode ?? "publicAuth") === "publicAuth") {
        const sb = (d.sourceHttpBase || "").trim() || activeConfig.stackTunHttpBase.trim();
        const sc = validateStackTunControlHttpUrl(sb);
        if ((d.sourceHttpBase || "").trim() && sc === "grpc_port_in_http_field") {
          errs.push(`${e.id}: ${t("tunnels.urls.sourceGrpcInControlField")}`);
        }
      }

      const p = typeof d.targetPort === "number" ? d.targetPort : 0;
      if (!Number.isFinite(p) || p <= 0 || p > 65535) errs.push(`${e.id}: ${t("tunnels.targetPort")}`);
    }
    const prof = canvasToTunnelProfiles(edges, nodes);
    if (prof.length === 0 && edges.length === 0) return t("tunnels.canvas.nothingToSave");
    if (prof.length === 0 && edges.length > 0) return errs.join("; ") || t("tunnels.canvas.badGraph");
    if (errs.length) return errs.join("; ");
    return null;
  };

  const profilesPreview = useMemo(
    () => stringifyProfilesPretty(canvasToTunnelProfiles(edges, nodes)),
    [edges, nodes],
  );

  const pushProfiles = async (): Promise<boolean> => {
    const msg = validateBeforePush();
    if (msg) {
      toast.error(msg);
      return false;
    }
    const prof = canvasToTunnelProfiles(edges, nodes);
    const body = stringifyProfilesPretty(prof);
    await invoke<string>("stack_tun_put_config", {
      baseUrl,
      bearer: bearerTok ? bearerTok : null,
      jsonBody: body,
    });
    return true;
  };

  const onSaveRemote = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    setBusy(true);
    onPersistHeaderFields();
    try {
      await pushProfiles();
      toast.success(tr("Профили на сервере", "Profiles pushed to stack-tun API"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onRun = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    setBusy(true);
    onPersistHeaderFields();
    try {
      await pushProfiles();
      await invoke<string>("stack_tun_reload_peers", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      const st = await invoke<string>("stack_tun_stats", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      setStatsRaw(st);
      toast.success(tr("Запуск / перезапуск", "Applied & peers reloaded"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const bmById = useMemo(() => new Map(bookmarks.map((b) => [b.id, b])), [bookmarks]);

  const onConnect = useCallback(
    (c: { source?: string | null; target?: string | null }) => {
      if (!c.source || !c.target || c.source === c.target) return;
      const targetNode = nodes.find((n) => n.id === c.target);
      if (targetNode?.type !== BOOKMARK_SERVER_NODE_TYPE) {
        toast.error(t("tunnels.canvas.targetMustBeServer"));
        return;
      }
      const bmId =
        targetNode?.type === BOOKMARK_SERVER_NODE_TYPE
          ? (targetNode.data as TunnelBookmarkNodeData).bookmarkId
          : "";
      const bm = bmId ? bmById.get(bmId) : undefined;
      const remoteGuess = bm ? guessStackTunGrpcFromGrpcUrl(bm.url) : "";
      const sourceNode = nodes.find((n) => n.id === c.source);
      const sourceHttpBase =
        sourceNode?.type === BOOKMARK_SERVER_NODE_TYPE
          ? guessStackTunHttpFromGrpcUrl((sourceNode.data as TunnelBookmarkNodeData).grpcUrl)
          : activeConfig.stackTunHttpBase;
      const e = blankEdgeBetween(
        c.target,
        { remoteGrpcEndpoint: remoteGuess, sourceHttpBase },
        c.source,
      );
      setEdges((cur) => [...cur, e]);
    },
    [activeConfig.stackTunHttpBase, bmById, nodes, setEdges, t],
  );
  const [paletteBmId, setPaletteBmId] = useState<string>("");

  const guardFlowDelete = useCallback(
    async (payload: Parameters<OnBeforeDelete<Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>, Edge<TunnelEdgeData>>>[0]) => {
      const delNs = payload.nodes;
      if (delNs.some((n) => n.id === LOCAL_PC_NODE_ID)) {
        toast.error(t("tunnels.canvas.cantDeletePc"));
        return false;
      }
      return true;
    },
    [t],
  );

  const addBookmarkNode = () => {
    const id = paletteBmId || bookmarks[0]?.id;
    const b = bookmarks.find((x) => x.id === id);
    if (!b) {
      toast.error(tr("Нет закладок сервера", "No saved server bookmarks — add under Connections."));
      return;
    }
    const nid = `server-${b.id}`;
    if (nodes.some((n) => n.id === nid)) {
      toast.info(tr("Уже на полотне", "Bookmark already placed on canvas"));
      return;
    }
    const next = [...nodes, defaultBookmarkNode(b)];
    setNodes(next);
  };

  const addTunnelEdge = () => {
    const srvNodes = nodes.filter((n) => n.type === BOOKMARK_SERVER_NODE_TYPE);
    let target =
      srvNodes.find((n) => (n.data as TunnelBookmarkNodeData)?.bookmarkId === paletteBmId) ??
      srvNodes[0];
    if (!target) {
      toast.error(tr("Добавьте сервер на полотно", "Drag a bookmark server onto the canvas first"));
      return;
    }
    const bm = bmById.get((target.data as TunnelBookmarkNodeData).bookmarkId);
    const remote = bm ? guessStackTunGrpcFromGrpcUrl(bm.url) : guessStackTunGrpcFromGrpcUrl("");
    const e = blankEdgeBetween(target.id, {
      remoteGrpcEndpoint: remote,
      sourceHttpBase: activeConfig.stackTunHttpBase,
      linkMode: "local",
    });
    setEdges((cur) => [...cur, e]);
  };

  const addServerRelayEdge = () => {
    const srvNodes = nodes.filter((n) => n.type === BOOKMARK_SERVER_NODE_TYPE);
    if (srvNodes.length < 2) {
      toast.error(tr("Нужно минимум два серверных узла", "Add at least two server nodes first"));
      return;
    }
    const target =
      srvNodes.find((n) => (n.data as TunnelBookmarkNodeData)?.bookmarkId === paletteBmId) ??
      srvNodes[1];
    const source = srvNodes.find((n) => n.id !== target.id) ?? srvNodes[0];
    if (!source || !target || source.id === target.id) {
      toast.error(tr("Выберите разные серверы", "Pick different server nodes"));
      return;
    }
    const bm = bmById.get((target.data as TunnelBookmarkNodeData).bookmarkId);
    const remote = bm ? guessStackTunGrpcFromGrpcUrl(bm.url) : guessStackTunGrpcFromGrpcUrl("");
    const sourceHttpBase = guessStackTunHttpFromGrpcUrl((source.data as TunnelBookmarkNodeData).grpcUrl);
    const e = blankEdgeBetween(
      target.id,
      { remoteGrpcEndpoint: remote, sourceHttpBase, linkMode: "publicAuth" },
      source.id,
    );
    setEdges((cur) => [...cur, e]);
  };

  const updateHeader = (partial: Partial<Pick<TunnelCanvasConfig, "stackTunHttpBase" | "stackTunBearer">>) =>
    mergeActivePreset((prev) => ({ ...prev, ...partial }));

  const renameActivePreset = (name: string) => mergeActivePreset((prev) => ({ ...prev, name }));

  const newPreset = () => {
    const id = newConfigId();
    const base = presets.configs[0];
    const c: TunnelCanvasConfig = {
      id,
      name: `${t("tunnels.canvas.preset")} ${presets.configs.length + 1}`,
      stackTunHttpBase: base?.stackTunHttpBase ?? "",
      stackTunBearer: base?.stackTunBearer ?? "",
      ...emptyCanvasPreset(),
    };
    setPresets((prevBlob) => {
      const blob: PresetsBlobV1 = { ...prevBlob, configs: [...prevBlob.configs, c], activeConfigId: id };
      savePresetsBlob(blob);
      return blob;
    });
    toast.success(tr("Новый пресет", "New preset"));
  };

  const deletePreset = () => {
    if (presets.configs.length <= 1) {
      toast.error(tr("Нельзя удалить последний пресет", "Cannot delete the last preset"));
      return;
    }
    setPresets((prevBlob) => {
      const nextConfigs = prevBlob.configs.filter((c) => c.id !== prevBlob.activeConfigId);
      const nextActive = nextConfigs[0]?.id ?? "";
      const blob: PresetsBlobV1 = {
        ...prevBlob,
        configs: nextConfigs,
        activeConfigId: nextActive,
      };
      savePresetsBlob(blob);
      return blob;
    });
  };

  const updateSelectedEdgeFields = (
    updater: Partial<TunnelEdgeData> | ((d: TunnelEdgeData) => TunnelEdgeData),
  ) => {
    if (picked.kind !== "edge" || !picked.edge?.id) return;
    const eid = picked.edge.id;
    setEdges((prev) =>
      prev.map((e) => {
        if (e.id !== eid || !e.data) return e;
        const merged =
          typeof updater === "function" ? updater(e.data as TunnelEdgeData) : { ...e.data, ...updater };
        const nextLbl = merged.listenAddr || "tunnel";
        return { ...e, label: typeof nextLbl === "string" ? nextLbl.slice(0, 40) : e.label, data: merged };
      }),
    );
  };

  const inspectedEdge = useMemo(() => {
    if (picked.kind !== "edge" || !picked.edge?.id) return undefined;
    const serverNodeIds = new Set(nodes.filter((n) => n.type === BOOKMARK_SERVER_NODE_TYPE).map((n) => n.id));
    return edges.find((e) => e.id === picked.edge!.id && serverNodeIds.has(e.target));
  }, [picked, edges, nodes]);

  const inspectedEdgeTitle = useMemo(() => {
    if (!inspectedEdge) return "";
    const labelFor = (id: string) => {
      if (id === LOCAL_PC_NODE_ID) return t("tunnels.canvas.localPc");
      const node = nodes.find((n) => n.id === id);
      if (node?.type === BOOKMARK_SERVER_NODE_TYPE) return (node.data as TunnelBookmarkNodeData).label;
      return id;
    };
    return `${labelFor(inspectedEdge.source)} -> ${labelFor(inspectedEdge.target)}`;
  }, [inspectedEdge, nodes, t]);

  const buildRequestsQuery = useCallback((): string => {
    const sp = new URLSearchParams();
    const lim = Number(reqFilter.limit) || 80;
    sp.set("limit", String(Math.min(Math.max(lim, 1), 500)));
    if (reqFilter.host.trim()) sp.set("host", reqFilter.host.trim());
    if (reqFilter.pathPart.trim()) sp.set("path", reqFilter.pathPart.trim());
    if (reqFilter.method.trim()) sp.set("method", reqFilter.method.trim());
    if (reqFilter.profileId.trim()) sp.set("profileId", reqFilter.profileId.trim());
    if (reqFilter.traceId.trim()) sp.set("traceId", reqFilter.traceId.trim());
    if (reqFilter.requestId.trim()) sp.set("requestId", reqFilter.requestId.trim());
    if (reqFilter.errorsOnly) sp.set("errorsOnly", "true");
    if (reqFilter.blockedOnly) sp.set("blockedOnly", "true");
    return `?${sp.toString()}`;
  }, [reqFilter]);

  const refreshRequests = useCallback(async () => {
    if (!isTauri() || !baseUrl.trim()) return;
    try {
      const q = buildRequestsQuery();
      const raw = await invoke<string>("stack_tun_requests_json", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
        query: q,
      });
      setReqLogRaw(raw);
    } catch (e) {
      toast.error(String(e));
    }
  }, [baseUrl, bearerTok, buildRequestsQuery]);

  useEffect(() => {
    if (!editorOpen || editorBusTab !== "requests" || !isTauri()) return;
    void refreshRequests();
  }, [editorOpen, editorBusTab, refreshRequests]);

  useIntervalWhenVisible(
    () => void refreshRequests(),
    2500,
    reqLive && editorOpen && editorBusTab === "requests",
  );

  const loadRoutesPack = useCallback(async () => {
    if (!isTauri() || !baseUrl.trim()) return;
    try {
      const raw = await invoke<string>("stack_tun_get_routes", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      const parsed = JSON.parse(raw) as { routes?: unknown };
      setRoutesEditor(JSON.stringify({ routes: parsed.routes ?? [] }, null, 2));
    } catch (e) {
      toast.error(String(e));
    }
  }, [baseUrl, bearerTok]);

  useEffect(() => {
    if (!editorOpen || editorBusTab !== "rules" || !isTauri()) return;
    void loadRoutesPack();
  }, [editorOpen, editorBusTab, loadRoutesPack]);

  const onLoadPeers = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    setBusy(true);
    try {
      const raw = await invoke<string>("stack_tun_list_peers", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
      });
      setPeersPretty(JSON.stringify(JSON.parse(raw), null, 2));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSaveRoutesPack = async () => {
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(routesEditor);
    } catch {
      toast.error(t("tunnels.routes.invalidJson"));
      return;
    }
    setBusy(true);
    try {
      await invoke<string>("stack_tun_put_routes", {
        baseUrl,
        bearer: bearerTok ? bearerTok : null,
        jsonBody: JSON.stringify(parsed),
      });
      toast.success(t("tunnels.routes.saved"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onGenerateAndRegisterPeer = async () => {
    if (!inspectedEdge?.data) return;
    if (!isTauri()) {
      toast.error(t("storage.tauriOnly"));
      return;
    }
    const primarySource =
      inspectedEdge.data.sourceHttpBase.trim() ||
      (inspectedEdge.source === LOCAL_PC_NODE_ID ? activeConfig.stackTunHttpBase : "");
    if (primarySource) {
      const pe = validateStackTunControlHttpUrl(primarySource.trim());
      if (pe === "grpc_port_in_http_field") {
        toast.error(t("tunnels.urls.sourceGrpcInControlField"));
        return;
      }
      if (pe === "bad_scheme") {
        toast.error(t("tunnels.urls.controlBad"));
        return;
      }
    }
    const targetBase = resolveTargetStackTunControlBase(
      activeConfig.stackTunHttpBase,
      inspectedEdge,
      nodes,
    ).trim();
    if (!targetBase) {
      toast.error(t("tunnels.canvas.targetHttpRequired"));
      return;
    }
    const targetErr = validateStackTunControlHttpUrl(targetBase);
    if (targetErr === "grpc_port_in_http_field") {
      toast.error(t("tunnels.urls.grpcPortInControl"));
      return;
    }
    if (targetErr === "bad_scheme" || targetErr === "empty") {
      toast.error(t("tunnels.urls.controlBad"));
      return;
    }
    const identityBases = collectSourceIdentityHttpBases(
      inspectedEdge,
      nodes,
      activeConfig.stackTunHttpBase,
    );
    const candidates = identityBases.filter((u) => validateStackTunControlHttpUrl(u) === null);
    if (!candidates.length) {
      toast.error(t("tunnels.canvas.sourceHttpRequired"));
      return;
    }
    setBusy(true);
    try {
      let publicKey = "";
      let usedBase = "";
      let lastErr = "";
      for (const tryBase of candidates) {
        try {
          const raw = await invoke<string>("stack_tun_identity_public_key", {
            baseUrl: tryBase,
            bearer: bearerTok ? bearerTok : null,
          });
          publicKey = parsePublicKeyJson(raw);
          usedBase = tryBase;
          break;
        } catch (e) {
          lastErr = String(e);
        }
      }
      if (!publicKey.trim()) {
        const hint =
          lastErr.includes("404") || lastErr.includes("Not Found")
            ? ` ${t("tunnels.canvas.identity404Hint")}`
            : "";
        toast.error(`${lastErr || t("tunnels.canvas.identityFetchFailed")}${hint}`);
        return;
      }
      await invoke<string>("stack_tun_authorize_peer", {
        baseUrl: targetBase,
        bearer: bearerTok ? bearerTok : null,
        publicKeyB64: publicKey,
      });
      updateSelectedEdgeFields((d) => ({
        ...d,
        linkMode: "publicAuth",
        sourceHttpBase: usedBase,
        connectorAllowPubkeyB64: appendTokenList(d.connectorAllowPubkeyB64, publicKey),
      }));
      toast.success(t("tunnels.canvas.peerRegistered"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mt-6 rounded-xl border border-border-subtle bg-panel p-6 shadow-card">
      <div className="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h2 className="text-xl font-semibold text-slate-100">{t("tunnels.canvas.title")}</h2>
          <p className="mt-2 max-w-3xl text-sm text-slate-400">{t("tunnels.canvas.intro")}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={busy}
            className="inline-flex items-center gap-2 rounded-lg border border-emerald-800/55 bg-emerald-950/40 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-emerald-100 hover:bg-emerald-950/60 disabled:opacity-50"
            onClick={() => void onHealth()}
          >
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Shield className="h-4 w-4" />}
            {t("tunnels.health")}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-end gap-3 border-b border-border-subtle pb-4">
        <label className="text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("tunnels.canvas.presets")}
          <select
            className="mt-1 block w-56 rounded-lg border border-border-subtle bg-black/35 px-2 py-1.5 text-sm text-slate-100"
            value={presets.activeConfigId}
            onChange={(e) => {
              const nid = e.target.value;
              setPresets((p) => {
                const blob = { ...p, activeConfigId: nid };
                savePresetsBlob(blob);
                return blob;
              });
            }}
          >
            {presets.configs.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className={`${btnMuted} mr-2`}
          onClick={newPreset}
        >
          <Plus className="h-4 w-4" />
          {t("tunnels.canvas.newPreset")}
        </button>
        <button type="button" className={`${btnMuted}`} onClick={deletePreset}>
          <Trash2 className="h-4 w-4" />
          {t("tunnels.canvas.deletePreset")}
        </button>
      </div>

      <label className="mt-4 block text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("tunnels.canvas.presetName")}
        <input
          className="mt-1 w-full max-w-md rounded-lg border border-border-subtle bg-black/35 px-3 py-2 text-sm text-slate-100"
          value={activeConfig.name}
          onChange={(e) => renameActivePreset(e.target.value)}
        />
      </label>

      <div className="mt-4 grid gap-4 md:grid-cols-2">
        <label className="block text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("tunnels.httpControl")}
          <input
            className="mt-1 w-full rounded-lg border border-border-subtle bg-black/35 px-3 py-2 text-sm font-mono text-slate-100"
            placeholder="http://127.0.0.1:9380"
            value={activeConfig.stackTunHttpBase}
            onChange={(e) => updateHeader({ stackTunHttpBase: e.target.value })}
            autoCapitalize="off"
            spellCheck={false}
          />
          <p className="mt-1 text-[10px] leading-relaxed text-slate-500">{t("tunnels.httpControlHint")}</p>
        </label>
        <label className="block text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("tunnels.restBearer")}
          <input
            className="mt-1 w-full rounded-lg border border-border-subtle bg-black/35 px-3 py-2 text-sm font-mono text-slate-100"
            placeholder="optional"
            type="password"
            value={activeConfig.stackTunBearer}
            onChange={(e) => updateHeader({ stackTunBearer: e.target.value })}
            autoCapitalize="off"
            spellCheck={false}
          />
        </label>
      </div>

      <div className="mt-4 flex flex-wrap gap-2">
        <button type="button" disabled={busy} className={`${btnWarn}`} onClick={() => void onFetchServer()}>
          <RotateCcw className={`h-4 w-4 ${busy ? "animate-spin" : ""}`} />
          {t("tunnels.canvas.fetchRebuild")}
        </button>
        <button type="button" disabled={busy} className={`${btnMuted}`} onClick={() => snapshotCanvasIntoPreset()}>
          <Save className="h-4 w-4" />
          {t("tunnels.canvas.saveLocal")}
        </button>
        <button type="button" disabled={busy} className={`${btnAmber}`} onClick={() => void onSaveRemote()}>
          <Save className="h-4 w-4" />
          {t("tunnels.canvas.saveRemote")}
        </button>
        <button type="button" disabled={busy} className={`${btnRun}`} onClick={() => void onRun()}>
          <Rocket className="h-4 w-4" />
          {t("tunnels.canvas.run")}
        </button>
        <button type="button" className={`${btnMuted}`} onClick={() => setEditorOpen(true)}>
          <PencilLine className="h-4 w-4" />
          {t("tunnels.canvas.openEditor")}
        </button>
      </div>

      <div className="mt-6 rounded-xl border border-border-subtle bg-black/30 p-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <p className="text-sm font-semibold text-slate-100">{t("tunnels.canvas.summaryTitle")}</p>
            <p className="mt-1 text-xs text-slate-500">
              {tr("Узлов: ", "Nodes: ")}
              {nodes.length} · {tr("связей: ", "edges: ")}
              {edges.length}
            </p>
          </div>
          <button type="button" className={`${btnRun}`} onClick={() => setEditorOpen(true)}>
            <PencilLine className="h-4 w-4" />
            {t("tunnels.canvas.openEditor")}
          </button>
        </div>
        <p className="mt-3 text-[11px] leading-relaxed text-slate-500">{t("tunnels.canvas.modalHint")}</p>
      </div>

      <ModalDialog
        open={editorOpen}
        onClose={() => {
          setEditorOpen(false);
          setEditorBusTab("schema");
        }}
        closeOnBackdrop={false}
        panelClassName="w-[min(1480px,calc(100vw-32px))]"
        className="flex items-center justify-center bg-black/80 p-4 backdrop-blur-sm"
      >
        <div className="max-h-[calc(100vh-40px)] overflow-hidden rounded-2xl border border-border-subtle bg-panel shadow-2xl">
          <div className="flex items-start justify-between gap-4 border-b border-border-subtle px-5 py-4">
            <div>
              <h3 className="text-lg font-semibold text-slate-100">{t("tunnels.canvas.editorTitle")}</h3>
              <p className="mt-1 max-w-3xl text-xs text-slate-500">{t("tunnels.canvas.editorIntro")}</p>
            </div>
            <button type="button" className={`${btnMuted} shrink-0`} onClick={() => setEditorOpen(false)}>
              <X className="h-4 w-4" />
              {t("storage.modalCancel")}
            </button>
          </div>

          <div className="flex flex-wrap gap-2 border-b border-border-subtle bg-black/25 px-5 py-2">
            <button
              type="button"
              className={`${btnMuted} ${editorBusTab === "schema" ? "border-amber-700/45 bg-amber-950/40" : ""}`}
              onClick={() => setEditorBusTab("schema")}
            >
              {t("tunnels.editorTab.schema")}
            </button>
            <button
              type="button"
              className={`${btnMuted} ${editorBusTab === "requests" ? "border-cyan-800/55 bg-cyan-950/35" : ""}`}
              onClick={() => setEditorBusTab("requests")}
            >
              {t("tunnels.editorTab.requests")}
            </button>
            <button
              type="button"
              className={`${btnMuted} ${editorBusTab === "rules" ? "border-purple-900/55 bg-purple-950/35" : ""}`}
              onClick={() => setEditorBusTab("rules")}
            >
              {t("tunnels.editorTab.rules")}
            </button>
          </div>

          <div className="max-h-[calc(100vh-160px)] overflow-auto p-5">
            {editorBusTab === "schema" ? (
              <>
                <div className="grid gap-6 lg:grid-cols-[minmax(240px,1fr)_3fr_minmax(260px,1fr)]">
                  <aside className="rounded-lg border border-border-subtle bg-black/35 p-3">
                    <div className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                      {t("tunnels.canvas.palette")}
                    </div>
                    <p className="mt-2 text-[11px] leading-relaxed text-slate-500">{t("tunnels.canvas.paletteHint")}</p>
                    <select
                      className="mt-3 w-full rounded-lg border border-border-subtle bg-black/45 px-2 py-1.5 text-xs text-slate-100"
                      value={paletteBmId}
                      onChange={(e) => setPaletteBmId(e.target.value)}
                    >
                      <option value="">{tr("Выберите закладку…", "Pick bookmark…")}</option>
                      {bookmarks.map((b) => (
                        <option key={b.id} value={b.id}>
                          {b.label || b.url}
                        </option>
                      ))}
                    </select>
                    <div className="mt-3 flex flex-col gap-2">
                      <button type="button" className={`${btnMuted} w-full justify-center py-2`} onClick={addBookmarkNode}>
                        <Plus className="h-4 w-4" />
                        {t("tunnels.canvas.addServer")}
                      </button>
                      <button type="button" className={`${btnMuted} w-full justify-center py-2`} onClick={addTunnelEdge}>
                        <Plus className="h-4 w-4" />
                        {t("tunnels.canvas.addTunnel")}
                      </button>
                      <button type="button" className={`${btnMuted} w-full justify-center py-2`} onClick={addServerRelayEdge}>
                        <Plus className="h-4 w-4" />
                        {t("tunnels.canvas.addRelay")}
                      </button>
                    </div>
                  </aside>

                  <div>
                    <Suspense
                      fallback={
                        <div className="flex h-[460px] items-center justify-center rounded-lg border border-border-subtle bg-black/40 text-sm text-slate-400">
                          {t("tunnels.canvas.loadingEditor")}
                        </div>
                      }
                    >
                      <StackTunnelsFlowEditor
                        nodes={nodes}
                        edges={edges}
                        setNodes={setNodes}
                        setEdges={setEdges}
                        onConnect={onConnect}
                        onNodeClick={(e, node) => {
                          e.stopPropagation();
                          setPicked({ kind: "node", node });
                        }}
                        onEdgeClick={(e, edge) => {
                          e.stopPropagation();
                          const targetNode = nodes.find((n) => n.id === edge.target);
                          if (targetNode?.type === BOOKMARK_SERVER_NODE_TYPE && edge.data)
                            setPicked({ kind: "edge", edge });
                          else setPicked({ kind: "none" });
                        }}
                        onPaneClick={() => {
                          setPicked({ kind: "none" });
                        }}
                        onBeforeDelete={guardFlowDelete}
                      />
                    </Suspense>
                    <p className="mt-2 text-[11px] text-slate-500">{t("tunnels.canvas.connectHint")}</p>
                    {statsRaw ? <StatsStrip raw={statsRaw} t={t} /> : null}
                  </div>

                  <aside className="rounded-lg border border-border-subtle bg-black/35 p-3">
                    <div className="text-xs font-semibold uppercase tracking-wide text-slate-500">
                      {t("tunnels.canvas.inspector")}
                    </div>
                    {inspectedEdge?.data ? (
                      <EdgeInspectorFields
                        d={inspectedEdge.data as TunnelEdgeData}
                        onChange={updateSelectedEdgeFields}
                        onGenerateAuth={onGenerateAndRegisterPeer}
                        busy={busy}
                        title={inspectedEdgeTitle}
                        t={t}
                      />
                    ) : picked.kind === "node" && picked.node ? (
                      <p className="mt-3 text-xs text-slate-400">
                        {picked.node.type === LOCAL_PC_NODE_TYPE
                          ? t("tunnels.canvas.localPcSelected")
                          : t("tunnels.canvas.remoteSelected")}
                      </p>
                    ) : (
                      <p className="mt-3 text-xs text-slate-400">{t("tunnels.canvas.selectEdge")}</p>
                    )}
                  </aside>
                </div>

                <label className="mt-6 block text-xs font-semibold uppercase tracking-wide text-slate-500">
                  {t("tunnels.canvas.previewProfiles")}
                  <textarea
                    className="mt-1 min-h-[180px] w-full rounded-xl border border-border-subtle bg-black/45 p-3 font-mono text-xs leading-relaxed text-emerald-100/95"
                    readOnly
                    spellCheck={false}
                    value={profilesPreview}
                  />
                </label>
                <p className="mt-3 text-[11px] leading-relaxed text-slate-500">{t("tunnels.docHint")}</p>
              </>
            ) : editorBusTab === "requests" ? (
              <div className="space-y-3">
                <p className="text-xs leading-relaxed text-slate-500">{t("tunnels.requests.intro")}</p>
                <div className="flex flex-wrap gap-2">
                  <button type="button" disabled={busy} className={btnMuted} onClick={() => void refreshRequests()}>
                    <RotateCcw className={`h-4 w-4 ${busy ? "animate-spin" : ""}`} />
                    {t("tunnels.requests.refresh")}
                  </button>
                  <label className={`${btnMuted} cursor-pointer gap-2`}>
                    <input
                      type="checkbox"
                      className="rounded border-white/20"
                      checked={reqLive}
                      onChange={(e) => setReqLive(e.target.checked)}
                    />
                    {t("tunnels.requests.live")}
                  </label>
                  <button type="button" disabled={busy} className={btnMuted} onClick={() => void onLoadPeers()}>
                    {t("tunnels.requests.loadPeers")}
                  </button>
                </div>

                <div className="grid gap-3 md:grid-cols-4">
                  <FieldSmall
                    label={t("tunnels.requests.limit")}
                    value={reqFilter.limit}
                    on={(v) => setReqFilter((p) => ({ ...p, limit: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.host")}
                    value={reqFilter.host}
                    on={(v) => setReqFilter((p) => ({ ...p, host: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.path")}
                    value={reqFilter.pathPart}
                    on={(v) => setReqFilter((p) => ({ ...p, pathPart: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.method")}
                    value={reqFilter.method}
                    on={(v) => setReqFilter((p) => ({ ...p, method: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.profileId")}
                    value={reqFilter.profileId}
                    on={(v) => setReqFilter((p) => ({ ...p, profileId: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.traceId")}
                    value={reqFilter.traceId}
                    on={(v) => setReqFilter((p) => ({ ...p, traceId: v }))}
                  />
                  <FieldSmall
                    label={t("tunnels.requests.requestId")}
                    value={reqFilter.requestId}
                    on={(v) => setReqFilter((p) => ({ ...p, requestId: v }))}
                  />
                  <label className="flex items-center gap-2 text-[11px] text-slate-400">
                    <input
                      type="checkbox"
                      checked={reqFilter.errorsOnly}
                      onChange={(e) => setReqFilter((p) => ({ ...p, errorsOnly: e.target.checked }))}
                    />
                    {t("tunnels.requests.errorsOnly")}
                  </label>
                  <label className="flex items-center gap-2 text-[11px] text-slate-400">
                    <input
                      type="checkbox"
                      checked={reqFilter.blockedOnly}
                      onChange={(e) => setReqFilter((p) => ({ ...p, blockedOnly: e.target.checked }))}
                    />
                    {t("tunnels.requests.blockedOnly")}
                  </label>
                </div>

                <pre className="max-h-[460px] overflow-auto rounded-xl border border-border-subtle bg-black/45 p-3 font-mono text-[11px] text-emerald-100/95">
                  {reqLogRaw}
                </pre>
                {peersPretty ? (
                  <>
                    <p className="text-xs font-semibold text-slate-300">{t("tunnels.requests.peersJson")}</p>
                    <pre className="max-h-52 overflow-auto rounded-lg border border-border-subtle bg-black/55 p-2 font-mono text-[10px] text-slate-300">
                      {peersPretty}
                    </pre>
                  </>
                ) : null}
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-xs leading-relaxed text-slate-500">{t("tunnels.routes.intro")}</p>
                <div className="flex flex-wrap gap-2">
                  <button type="button" disabled={busy} className={btnMuted} onClick={() => void loadRoutesPack()}>
                    <RotateCcw className={`h-4 w-4 ${busy ? "animate-spin" : ""}`} />
                    {t("tunnels.routes.reload")}
                  </button>
                  <button type="button" disabled={busy} className={`${btnAmber}`} onClick={() => void onSaveRoutesPack()}>
                    <Save className="h-4 w-4" />
                    {t("tunnels.routes.save")}
                  </button>
                </div>
                <textarea
                  className="min-h-[420px] w-full rounded-xl border border-border-subtle bg-black/45 p-3 font-mono text-xs leading-relaxed text-slate-100"
                  spellCheck={false}
                  value={routesEditor}
                  onChange={(e) => setRoutesEditor(e.target.value)}
                />
              </div>
            )}
          </div>
        </div>
      </ModalDialog>
    </section>
  );
}

function StatsStrip(props: { raw: string; t: (key: string) => string }) {
  const parsed = parseStatsJson(props.raw);
  if (!parsed) {
    return <pre className="mt-2 max-h-24 overflow-auto text-[11px] text-slate-500">{props.raw}</pre>;
  }
  const row = ([k, v]: [string, unknown]) =>
    `${k}: ${typeof v === "number" ? v.toLocaleString?.() ?? String(v) : String(v ?? "—")}`;
  return (
    <div className="mt-2 rounded-lg border border-white/10 bg-black/55 px-3 py-2 text-[11px] text-slate-300">
      <div className="font-semibold text-slate-400">{props.t("tunnels.canvas.lastStats")}</div>
      <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
        {parsed.listenerAccepts !== undefined ? <span>{row(["listenerAccepts", parsed.listenerAccepts])}</span> : null}
        {parsed.connectorPulls !== undefined ? <span>{row(["connectorPulls", parsed.connectorPulls])}</span> : null}
        {parsed.relayCompleted !== undefined ? <span>{row(["relayCompleted", parsed.relayCompleted])}</span> : null}
        {parsed.relayErrors !== undefined ? <span>{row(["relayErrors", parsed.relayErrors])}</span> : null}
        {parsed.requestBusReceived !== undefined ? (
          <span>{row(["requestBusReceived", parsed.requestBusReceived])}</span>
        ) : null}
        {parsed.requestBusBlocked !== undefined ? (
          <span>{row(["requestBusBlocked", parsed.requestBusBlocked])}</span>
        ) : null}
        {parsed.requestBusCompleted !== undefined ? (
          <span>{row(["requestBusCompleted", parsed.requestBusCompleted])}</span>
        ) : null}
        {parsed.requestBusErrors !== undefined ? (
          <span>{row(["requestBusErrors", parsed.requestBusErrors])}</span>
        ) : null}
      </div>
    </div>
  );
}

function EdgeInspectorFields(props: {
  d: TunnelEdgeData;
  onChange: (patch: Partial<TunnelEdgeData>) => void;
  onGenerateAuth: () => void | Promise<void>;
  busy: boolean;
  title: string;
  t: (k: string) => string;
}) {
  const { d, onChange, onGenerateAuth, busy, title, t } = props;
  const linkMode = d.linkMode ?? "publicAuth";
  return (
    <div className="mt-3 flex flex-col gap-2">
      <div className="rounded-lg border border-white/10 bg-black/45 px-2 py-1.5 text-[11px] text-slate-400">
        <span className="font-semibold text-slate-300">{t("tunnels.canvas.edgeRoute")}:</span>{" "}
        <span className="font-mono">{title || "—"}</span>
      </div>
      <label className="text-[11px] text-slate-500">
        {t("tunnels.canvas.linkMode")}
        <select
          className="mt-1 w-full rounded border border-white/10 bg-black/55 px-2 py-1 text-sm text-slate-100"
          value={linkMode}
          onChange={(e) =>
            onChange({
              linkMode: e.target.value === "local" ? "local" : "publicAuth",
            })
          }
        >
          <option value="local">{t("tunnels.canvas.linkModeLocal")}</option>
          <option value="publicAuth">{t("tunnels.canvas.linkModePublicAuth")}</option>
        </select>
      </label>
      <label className="text-[11px] text-slate-500">
        {t("tunnels.canvas.tunnelMode")}
        <select
          className="mt-1 w-full rounded border border-white/10 bg-black/55 px-2 py-1 text-sm text-slate-100"
          value={d.tunnelMode ?? "tcpRelay"}
          onChange={(e) =>
            onChange({
              tunnelMode: (e.target.value === "requestBus" ? "requestBus" : "tcpRelay") as TunnelWireMode,
            })
          }
        >
          <option value="tcpRelay">{t("tunnels.canvas.tunnelModeTcp")}</option>
          <option value="requestBus">{t("tunnels.canvas.tunnelModeBus")}</option>
        </select>
      </label>
      <p className="text-[10px] leading-relaxed text-slate-500">{t("tunnels.canvas.tunnelModeHint")}</p>
      <Field
        lab={t("tunnels.listenAddr")}
        val={d.listenAddr}
        on={(v) => onChange({ listenAddr: v })}
      />
      <Field
        lab={t("tunnels.canvas.remoteGrpcRelay")}
        val={d.remoteGrpcEndpoint}
        mono
        on={(v) => onChange({ remoteGrpcEndpoint: v })}
      />
      <p className="text-[10px] leading-relaxed text-slate-500">{t("tunnels.canvas.remoteGrpcHint")}</p>
      <Field lab={t("tunnels.targetHost")} val={d.targetHost} on={(v) => onChange({ targetHost: v })} />
      <Field
        lab={t("tunnels.canvas.sourceHttpBase")}
        val={d.sourceHttpBase ?? ""}
        mono
        on={(v) => onChange({ sourceHttpBase: v })}
      />
      <p className="text-[10px] leading-relaxed text-slate-500">{t("tunnels.canvas.sourceHttpHint")}</p>
      <label className="text-[11px] text-slate-500">
        {t("tunnels.targetPort")}
        <input
          type="number"
          className="mt-1 w-full rounded border border-white/10 bg-black/55 px-2 py-1 text-sm text-slate-100"
          value={Number.isFinite(d.targetPort) ? d.targetPort : 0}
          onChange={(e) => onChange({ targetPort: Number(e.target.value) || 0 })}
        />
      </label>
      <label className="flex cursor-pointer items-center gap-2 text-[11px] text-slate-400">
        <input
          type="checkbox"
          checked={Boolean(d.enabled)}
          onChange={(e) => onChange({ enabled: e.target.checked })}
        />
        {t("tunnels.canvas.enabled")}
      </label>
      <label className="text-[11px] text-slate-500">
        {t("tunnels.canvas.allowPubkeys")}
        <textarea
          className="mt-1 min-h-[56px] w-full rounded border border-white/10 bg-black/55 p-2 font-mono text-[11px]"
          value={d.connectorAllowPubkeyB64}
          onChange={(e) => onChange({ connectorAllowPubkeyB64: e.target.value })}
        />
      </label>
      <button
        type="button"
        disabled={busy}
        className={`${btnAmber} justify-center py-2`}
        onClick={() => void onGenerateAuth()}
      >
        {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Shield className="h-4 w-4" />}
        {t("tunnels.canvas.generateAuthKey")}
      </button>
      <p className="text-[11px] leading-relaxed text-slate-500">{t("tunnels.canvas.generateAuthHint")}</p>
    </div>
  );
}

function Field(props: {
  lab: string;
  val: string;
  mono?: boolean;
  on: (v: string) => void;
}) {
  return (
    <label className="text-[11px] text-slate-500">
      {props.lab}
      <input
        className={`mt-1 w-full rounded border border-white/10 bg-black/55 px-2 py-1 text-sm text-slate-100 ${
          props.mono ? "font-mono" : ""
        }`}
        value={props.val}
        onChange={(e) => props.on(e.target.value)}
      />
    </label>
  );
}

function FieldSmall(props: { label: string; value: string; on: (v: string) => void }) {
  return (
    <label className="flex flex-col gap-0.5 text-[10px] text-slate-500">
      <span className="font-semibold uppercase tracking-wide text-slate-500">{props.label}</span>
      <input
        className="rounded border border-white/10 bg-black/55 px-2 py-1 text-xs text-slate-100"
        value={props.value}
        onChange={(e) => props.on(e.target.value)}
      />
    </label>
  );
}

const btnMuted =
  "inline-flex items-center gap-2 rounded-lg border border-slate-800 bg-slate-950/55 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-slate-200 hover:bg-slate-900/85 disabled:opacity-50";
const btnWarn =
  "inline-flex items-center gap-2 rounded-lg border border-red-900/55 bg-red-950/55 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-red-50 hover:bg-red-900/65 disabled:opacity-50";
const btnAmber =
  "inline-flex items-center gap-2 rounded-lg border border-amber-800/55 bg-amber-950/40 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-amber-50 hover:bg-amber-950/58 disabled:opacity-50";
const btnRun =
  "inline-flex items-center gap-2 rounded-lg border border-orange-900/55 bg-orange-950/55 px-3 py-2 text-xs font-semibold uppercase tracking-wide text-orange-50 hover:bg-orange-900/62 disabled:opacity-50";
