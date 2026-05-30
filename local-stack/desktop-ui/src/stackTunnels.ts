/**
 * Types and converters for stack-tun visual canvas ↔ REST `profiles` payloads.
 */

import type { Edge, Node } from "@xyflow/react";

/** Matches server `TunnelMode` (`tcpRelay` | `requestBus`). */
export type TunnelWireMode = "tcpRelay" | "requestBus";

/** Matches server `TunnelLinkKind` (`local` | `publicAuth`). Canvas uses the same literals. */
export type TunnelLinkKind = "local" | "publicAuth";

/** Matches `TunnelRole` in server-stack/stack-tun-api (`listen` | `connector`). */
export type StackTunTunnelRole = "listen" | "connector";

/** Matches `TunnelProfile` JSON (`camelCase` serde). */
export type StackTunTunnelProfile = {
  id: string;
  name: string;
  role: StackTunTunnelRole;
  mode?: TunnelWireMode;
  linkKind?: TunnelLinkKind;
  sourceNodeId?: string | null;
  targetNodeId?: string | null;
  routeTags?: string[];
  allowedHosts?: string[];
  allowedPaths?: string[];
  routePriority?: number | null;
  defaultBusDecision?: "allow" | "deny" | "forward" | "localHandle" | null;
  listenAddr?: string | null;
  remoteGrpcEndpoint?: string | null;
  listenProfileId?: string | null;
  targetHost: string;
  targetPort: number;
  maxPendingStreams: number;
  streamOfferTtlSecs: number;
  pullWaitMs: number;
  connectorAllowPubkeyB64: string[];
  enabled: boolean;
};

export type StackTunPersistRoot = {
  version?: number;
  profiles: StackTunTunnelProfile[];
};

export type ServerBookmarkBrief = {
  id: string;
  label: string;
  url: string;
};

export type TunnelLinkMode = "local" | "publicAuth";

export type StackTunRouteRule = {
  id: string;
  priority?: number;
  hostContains?: string | null;
  pathPrefix?: string | null;
  method?: string | null;
  decision: "allow" | "deny" | "forward" | "localHandle" | "queue";
  forwardUrl?: string | null;
  localHost?: string | null;
  localPort?: number | null;
};

export type TunnelRequestLogEntry = {
  tsUnixMs?: number;
  hopId?: number;
  requestId: string;
  traceId?: string;
  sourceNodeId?: string;
  targetNodeId?: string;
  profileId?: string;
  method?: string;
  host?: string;
  path?: string;
  status: number;
  decision?: string;
  error?: string | null;
  bytesIn?: number;
  bytesOut?: number;
};

/** Per-edge tunnel parameters (shown in inspector). */
export type TunnelEdgeData = {
  listenProfileId: string;
  connectorProfileId: string;
  /** Public/listen bind on the stack-tun host, e.g. `0.0.0.0:9000` */
  listenAddr: string;
  /** Remote stack-tun gRPC base — data plane Relay (typically `:9381`). */
  remoteGrpcEndpoint: string;
  /** Local upstream for the connector relay. */
  targetHost: string;
  targetPort: number;
  maxPendingStreams: number;
  streamOfferTtlSecs: number;
  pullWaitMs: number;
  enabled: boolean;
  connectorAllowPubkeyB64: string;
  /** local = trusted/local path; publicAuth = public listener + explicit connector pubkey allow-list. */
  linkMode: TunnelLinkMode;
  /** HTTP control-plane of the connector/source stack-tun node (:9380), used to fetch its public key. */
  sourceHttpBase: string;
  /** tcpRelay = raw TunnelStream relay; requestBus = structured envelopes + routing. */
  tunnelMode: TunnelWireMode;
};

export type TunnelLocalPcNodeData = Record<string, never>;

export type TunnelBookmarkNodeData = {
  bookmarkId: string;
  label: string;
  grpcUrl: string;
};

export const LOCAL_PC_NODE_TYPE = "stackTunLocalPc";
export const BOOKMARK_SERVER_NODE_TYPE = "stackTunBookmarkServer";

export const LOCAL_PC_NODE_ID = "local-pc-node";

/** Visual document stored in localStorage (named presets). */
export type TunnelCanvasConfig = {
  id: string;
  name: string;
  stackTunHttpBase: string;
  stackTunBearer: string;
  rfNodes: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[];
  rfEdges: Edge<TunnelEdgeData>[];
};

const DEFAULT_PROFILE_NUM = {
  maxPendingStreams: 128,
  streamOfferTtlSecs: 300,
  pullWaitMs: 30000,
} as const;

function newStableId(prefix: string): string {
  const u =
    typeof globalThis.crypto !== "undefined" && "randomUUID" in globalThis.crypto
      ? globalThis.crypto.randomUUID()
      : `${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`;
  return `${prefix}_${u}`;
}

export function normalizeStackTunUrl(url: string): string {
  return url.trim().replace(/\/+$/, "");
}

/** Control-plane URLs must speak REST (stack-tun HTTP API, usually `:9380`). */
export function validateStackTunControlHttpUrl(
  url: string,
): "empty" | "bad_scheme" | "grpc_port_in_http_field" | null {
  const t = normalizeStackTunUrl(url);
  if (!t) return "empty";
  try {
    const protoFix = t.replace(/^grpc(\+tls)?:\/\//i, "http://");
    const u = new URL(protoFix);
    if (!/^https?:$/i.test(u.protocol)) return "bad_scheme";
    if (!u.hostname) return "bad_scheme";
    if (!u.port) return null;
    const port = parseInt(u.port, 10);
    if (Number.isFinite(port) && port === 9381) return "grpc_port_in_http_field";
    return null;
  } catch {
    return "bad_scheme";
  }
}

/** Relay gRPC usually `:9381` (not `:9380` REST API). */
export function validateStackTunRelayGrpcUrl(
  url: string,
): "empty" | "bad_scheme" | "http_port_in_grpc_field" | null {
  const t = normalizeStackTunUrl(url);
  if (!t) return "empty";
  try {
    const protoFix = t.replace(/^grpc(\+tls)?:\/\//i, "http://");
    const u = new URL(protoFix);
    if (!/^https?:$/i.test(u.protocol)) return "bad_scheme";
    if (!u.hostname) return "bad_scheme";
    if (!u.port) return null;
    const port = parseInt(u.port, 10);
    if (Number.isFinite(port) && port === 9380) return "http_port_in_grpc_field";
    return null;
  } catch {
    return "bad_scheme";
  }
}

/** Best-effort: stack-tun gRPC bind often uses `:9381` while bookmarks store deploy gRPC. */
export function guessStackTunGrpcFromGrpcUrl(grpcUrl: string): string {
  const trimmed = grpcUrl.trim();
  if (!trimmed) return "http://127.0.0.1:9381";
  try {
    const normalized = trimmed.replace(/^grpc\+tls:/i, "https:").replace(/^grpc:/i, "http:");
    const u = new URL(normalized);
    let port = u.port || "";
    const host = u.hostname || "127.0.0.1";
    const isSecure = /^grpcs:/i.test(trimmed) || /^grpc\+tls:/i.test(trimmed) || u.protocol === "https:";
    const scheme = isSecure ? "https" : "http";
    if (!port || port === "443" || port === "50051") {
      port = "9381";
    }
    return `${scheme}://${host}:${port}`;
  } catch {
    const m = trimmed.match(/^(?:[^:]+:\/\/)?(?:[^:]+:\/\/)?([^/:]+)/);
    const host = m?.[1] ?? "127.0.0.1";
    return `http://${host}:9381`;
  }
}

export function guessStackTunHttpFromGrpcUrl(grpcUrl: string): string {
  const grpc = guessStackTunGrpcFromGrpcUrl(grpcUrl);
  try {
    const u = new URL(grpc);
    u.port = "9380";
    return u.toString().replace(/\/$/, "");
  } catch {
    return "http://127.0.0.1:9380";
  }
}

function extractHostRough(url: string): string | null {
  const trimmed = url.trim();
  try {
    const normalized = trimmed.replace(/^grpc\+tls:/i, "https:").replace(/^grpc:/i, "http:");
    const u = new URL(normalized);
    return u.hostname || null;
  } catch {
    const m = trimmed.match(/^[^/]*\/\/([^/:]+)/);
    return m?.[1]?.trim() || null;
  }
}

/** Does `bookmark.url` correspond to stack-tun gRPC base or deploy URL on same host? */
export function bookmarkMatchesGrpcBase(bookmark: ServerBookmarkBrief, remoteGrpcEndpoint: string): boolean {
  const eb = bookmark.url.trim().toLowerCase();
  const gr = remoteGrpcEndpoint.trim().toLowerCase();
  if (!eb || !gr) return false;
  if (eb === gr) return true;
  const hb = extractHostRough(bookmark.url);
  const hg = extractHostRough(remoteGrpcEndpoint.replace(/^https?:\/\//, "grpc://"));
  if (!hb || !hg) return false;
  return hb === hg;
}

export function blankEdgeBetween(
  targetNodeId: string,
  overrides: Partial<TunnelEdgeData> = {},
  sourceNodeId: string = LOCAL_PC_NODE_ID,
): Edge<TunnelEdgeData> {
  const listenProfileId = newStableId("listen");
  const connectorProfileId = newStableId("conn");
  return {
    id: newStableId("edge"),
    source: sourceNodeId,
    target: targetNodeId,
    type: "default",
    label: overrides.listenAddr ?? "tunnel",
    data: {
      listenProfileId,
      connectorProfileId,
      listenAddr: overrides.listenAddr ?? "0.0.0.0:9000",
      remoteGrpcEndpoint: overrides.remoteGrpcEndpoint ?? "",
      targetHost: overrides.targetHost ?? "127.0.0.1",
      targetPort: overrides.targetPort ?? 8080,
      maxPendingStreams: overrides.maxPendingStreams ?? DEFAULT_PROFILE_NUM.maxPendingStreams,
      streamOfferTtlSecs: overrides.streamOfferTtlSecs ?? DEFAULT_PROFILE_NUM.streamOfferTtlSecs,
      pullWaitMs: overrides.pullWaitMs ?? DEFAULT_PROFILE_NUM.pullWaitMs,
      enabled: overrides.enabled !== false,
      connectorAllowPubkeyB64:
        overrides.connectorAllowPubkeyB64 ??
        "",
      linkMode: overrides.linkMode ?? "publicAuth",
      sourceHttpBase: overrides.sourceHttpBase ?? "",
      tunnelMode: overrides.tunnelMode ?? "tcpRelay",
    },
    markerEnd: { type: "arrowclosed", color: "rgba(251,146,60,0.85)" },
  };
}

export function defaultBookmarkNode(bookmark: ServerBookmarkBrief): Node<TunnelBookmarkNodeData> {
  return {
    id: `server-${bookmark.id}`,
    type: BOOKMARK_SERVER_NODE_TYPE,
    position: { x: 420, y: 140 },
    data: {
      bookmarkId: bookmark.id,
      label: bookmark.label || bookmark.url,
      grpcUrl: bookmark.url.trim(),
    },
  };
}

export function defaultLocalPcNode(): Node<TunnelLocalPcNodeData> {
  return {
    id: LOCAL_PC_NODE_ID,
    type: LOCAL_PC_NODE_TYPE,
    position: { x: 80, y: 140 },
    data: {},
    draggable: true,
    selectable: true,
  };
}

/** Build two stack-tun profiles from one canvas edge (+ bookmark GRPC for default remote endpoint). */
export function edgeToTunnelProfiles(
  edge: Edge<TunnelEdgeData>,
  bookmarkGrpcUrl: string,
): StackTunTunnelProfile[] {
  const d = edge.data;
  if (!d) return [];
  const remote =
    (d.remoteGrpcEndpoint || "").trim() || guessStackTunGrpcFromGrpcUrl(bookmarkGrpcUrl || "");
  const listenName = `listen:${d.listenAddr || "listen"}`;
  const connName = `conn:${d.targetHost}:${d.targetPort}`;
  const allow = (d.linkMode === "publicAuth" ? d.connectorAllowPubkeyB64 : "")
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);

  const wireMode = d.tunnelMode ?? "tcpRelay";
  const linkKind = d.linkMode;
  const sourceNodeRef = edge.source;
  const targetNodeRef = edge.target;

  const listen: StackTunTunnelProfile = {
    id: d.listenProfileId,
    name: listenName.slice(0, 120),
    role: "listen",
    mode: wireMode,
    linkKind,
    sourceNodeId: sourceNodeRef,
    targetNodeId: targetNodeRef,
    listenAddr:
      wireMode === "tcpRelay"
        ? d.listenAddr.trim()
        : ((d.listenAddr || "").trim() || null),
    remoteGrpcEndpoint: null,
    listenProfileId: null,
    targetHost: "",
    targetPort: 0,
    maxPendingStreams: d.maxPendingStreams,
    streamOfferTtlSecs: d.streamOfferTtlSecs,
    pullWaitMs: d.pullWaitMs,
    connectorAllowPubkeyB64: allow,
    enabled: d.enabled,
  };

  const connector: StackTunTunnelProfile = {
    id: d.connectorProfileId,
    name: connName.slice(0, 120),
    role: "connector",
    mode: wireMode,
    linkKind,
    sourceNodeId: sourceNodeRef,
    targetNodeId: targetNodeRef,
    listenAddr: null,
    remoteGrpcEndpoint: remote,
    listenProfileId: d.listenProfileId,
    targetHost: d.targetHost.trim() || "127.0.0.1",
    targetPort:
      typeof d.targetPort === "number" && Number.isFinite(d.targetPort) ? Number(d.targetPort) : 0,
    maxPendingStreams: d.maxPendingStreams,
    streamOfferTtlSecs: d.streamOfferTtlSecs,
    pullWaitMs: d.pullWaitMs,
    connectorAllowPubkeyB64: allow,
    enabled: d.enabled,
  };

  return [listen, connector];
}

/** All edges → flat profile list for `PUT /api/v1/config`. */
export function canvasToTunnelProfiles(
  edges: Edge<TunnelEdgeData>[],
  nodes: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[],
): StackTunTunnelProfile[] {
  const byBm = new Map<string, TunnelBookmarkNodeData>();
  for (const n of nodes) {
    if (n.type === BOOKMARK_SERVER_NODE_TYPE && "bookmarkId" in (n.data as object)) {
      const bd = n.data as TunnelBookmarkNodeData;
      byBm.set(n.id, bd);
    }
  }

  const out: StackTunTunnelProfile[] = [];
  const seen = new Set<string>();

  for (const e of edges) {
    if (e.source === e.target) continue;
    const bm = byBm.get(e.target);
    if (!bm || !e.data) continue;
    const pair = edgeToTunnelProfiles(e, bm.grpcUrl);
    for (const p of pair) {
      if (seen.has(p.id)) continue;
      seen.add(p.id);
      out.push(p);
    }
  }
  return out;
}

function parseProfilesJson(raw: string): StackTunTunnelProfile[] {
  const root = JSON.parse(raw) as StackTunPersistRoot | StackTunTunnelProfile[];
  if (Array.isArray(root)) return root;
  if (root && Array.isArray(root.profiles)) return root.profiles as StackTunTunnelProfile[];
  return [];
}

/** Best-effort import: pair listen profiles with connectors referencing `listenProfileId`. */
export function tunnelProfilesToCanvas(
  rawJson: string,
  bookmarks: ServerBookmarkBrief[],
): Pick<TunnelCanvasConfig, "rfNodes" | "rfEdges"> {
  let profiles: StackTunTunnelProfile[] = [];
  try {
    profiles = parseProfilesJson(rawJson);
  } catch {
    return {
      rfNodes: [defaultLocalPcNode()],
      rfEdges: [],
    };
  }

  const listens = profiles.filter((p) => p.role === "listen");
  const connectors = profiles.filter((p) => p.role === "connector");
  const listenById = new Map(listens.map((l) => [l.id, l]));

  type Pair = {
    listen: StackTunTunnelProfile;
    connector: StackTunTunnelProfile;
  };
  const pairs: Pair[] = [];

  for (const c of connectors) {
    const lid = (c.listenProfileId || "").trim();
    if (!lid) continue;
    const lp = listenById.get(lid);
    if (!lp) continue;
    if (!(c.remoteGrpcEndpoint || "").trim()) continue;
    if ((lp.mode ?? "tcpRelay") === "tcpRelay" && !(lp.listenAddr || "").trim()) continue;
    pairs.push({ listen: lp, connector: c });
  }

  /** Orphan listens (no connector in same payload): still visualize as dangling candidate. */

  const serverNodesByKey = new Map<string, Node<TunnelBookmarkNodeData>>();
  let yBookmark = 40;

  function ensureServerForGrpc(remoteGrpc: string): Node<TunnelBookmarkNodeData> {
    const guessed = guessStackTunGrpcFromGrpcUrl(remoteGrpc);
    const key =
      guessed ||
      extractHostRough(remoteGrpc) ||
      remoteGrpc.trim() ||
      "unknown-host";
    const existing = serverNodesByKey.get(key);
    if (existing) return existing;

    let bookmark = bookmarks.find((b) => bookmarkMatchesGrpcBase(b, remoteGrpc));
    if (!bookmark) {
      const hostHint = extractHostRough(remoteGrpc) || key;
      bookmark = {
        id: `synthetic:${key}`,
        label: remoteGrpc.includes("9381") ? `stack-tun @ ${hostHint}` : hostHint,
        url: remoteGrpc,
      };
    }

    const n = defaultBookmarkNode(bookmark);
    n.position = { x: 460, y: yBookmark };
    yBookmark += 160;
    serverNodesByKey.set(key, n);
    return n;
  }

  const rfEdges: Edge<TunnelEdgeData>[] = [];
  let idx = 0;
  for (const { listen, connector } of pairs) {
    const srv = ensureServerForGrpc(connector.remoteGrpcEndpoint!.trim());
    const edgeSource =
      (listen.sourceNodeId || connector.sourceNodeId || "").trim() || LOCAL_PC_NODE_ID;

    rfEdges.push({
      id: `import-edge-${listen.id}-${connector.id}-${idx++}`,
      source: edgeSource,
      target: srv.id,
      type: "default",
      data: {
        listenProfileId: listen.id,
        connectorProfileId: connector.id,
        listenAddr: (listen.listenAddr || "").trim(),
        remoteGrpcEndpoint: (connector.remoteGrpcEndpoint || "").trim(),
        targetHost: connector.targetHost || "127.0.0.1",
        targetPort: connector.targetPort || 0,
        maxPendingStreams: listen.maxPendingStreams ?? DEFAULT_PROFILE_NUM.maxPendingStreams,
        streamOfferTtlSecs: listen.streamOfferTtlSecs ?? DEFAULT_PROFILE_NUM.streamOfferTtlSecs,
        pullWaitMs: listen.pullWaitMs ?? DEFAULT_PROFILE_NUM.pullWaitMs,
        enabled: listen.enabled !== false && connector.enabled !== false,
        connectorAllowPubkeyB64: [...(listen.connectorAllowPubkeyB64 || [])].join("\n"),
        linkMode:
          (listen.linkKind as TunnelLinkMode) ??
          (((listen.connectorAllowPubkeyB64 || []).length > 0 ? "publicAuth" : "local") as TunnelLinkMode),
        sourceHttpBase: "",
        tunnelMode: (listen.mode as TunnelWireMode) ?? "tcpRelay",
      },
      markerEnd: { type: "arrowclosed", color: "rgba(251,146,60,0.85)" },
    });
  }

  const rfNodes: Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>[] = [
    defaultLocalPcNode(),
    ...[...serverNodesByKey.values()],
  ];

  return { rfNodes, rfEdges };
}

export function emptyCanvasPreset(): Pick<TunnelCanvasConfig, "rfNodes" | "rfEdges"> {
  return {
    rfNodes: [defaultLocalPcNode()],
    rfEdges: [],
  };
}

export function stringifyProfilesPretty(profiles: StackTunTunnelProfile[]): string {
  return JSON.stringify({ profiles }, null, 2);
}
