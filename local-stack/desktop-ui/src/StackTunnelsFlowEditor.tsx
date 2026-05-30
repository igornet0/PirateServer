import type React from "react";
import {
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type Node,
  type NodeProps,
  type OnBeforeDelete,
  type OnEdgesChange,
  type OnNodesChange,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useI18n } from "./i18n";
import {
  BOOKMARK_SERVER_NODE_TYPE,
  LOCAL_PC_NODE_ID,
  LOCAL_PC_NODE_TYPE,
  guessStackTunGrpcFromGrpcUrl,
  type TunnelBookmarkNodeData,
  type TunnelEdgeData,
  type TunnelLocalPcNodeData,
} from "./stackTunnels";

type TunnelNode = Node<TunnelLocalPcNodeData | TunnelBookmarkNodeData>;
type TunnelEdge = Edge<TunnelEdgeData>;

function TunnelLocalPcNode(props: NodeProps<Node<TunnelLocalPcNodeData>>) {
  const { language, t } = useI18n();
  return (
    <div
      className={`rounded-xl border border-emerald-800/55 bg-emerald-950/45 px-4 py-3 shadow-lg ${
        props.selected ? "ring-2 ring-emerald-500/65" : ""
      }`}
    >
      <Handle type="source" position={Position.Right} className="!h-3 !w-3 !border-amber-600/55 !bg-amber-500/95" />
      <div className="text-[11px] font-semibold uppercase tracking-wide text-emerald-200/95">
        {t("tunnels.canvas.localPc")}
      </div>
      <div className="mt-1 text-xs text-emerald-100/85">
        {language === "ru"
          ? "Connector-профиль: проброс TCP на вашем ПК через stack-tun gRPC удалённой стороны."
          : "Connector profile: relays bytes to localhost/LAN upstream on this desktop."}
      </div>
    </div>
  );
}

function TunnelBookmarkServerNode(props: NodeProps<Node<TunnelBookmarkNodeData>>) {
  const { t } = useI18n();
  const data = props.data;
  const grpcUrl = typeof data?.grpcUrl === "string" ? data.grpcUrl : "";
  const hint = guessStackTunGrpcFromGrpcUrl(grpcUrl);
  return (
    <div
      className={`max-w-[220px] rounded-xl border border-amber-800/55 bg-amber-950/35 px-3 py-2.5 shadow-lg ${
        props.selected ? "ring-2 ring-amber-500/55" : ""
      }`}
    >
      <Handle type="target" position={Position.Left} className="!h-3 !w-3 !border-orange-700/65 !bg-orange-500/95" />
      <Handle type="source" position={Position.Right} className="!h-3 !w-3 !border-cyan-700/65 !bg-cyan-400/95" />
      <div className="text-[11px] font-semibold uppercase tracking-wide text-amber-200/90">{t("tunnels.canvas.remote")}</div>
      <div className="mt-1 truncate text-sm font-semibold text-amber-50/98" title={data?.label}>
        {data?.label ?? "—"}
      </div>
      <div className="mt-1 truncate font-mono text-[10px] text-slate-400" title={grpcUrl}>
        {hint}
      </div>
    </div>
  );
}

const nodeTypes = {
  [LOCAL_PC_NODE_TYPE]: TunnelLocalPcNode,
  [BOOKMARK_SERVER_NODE_TYPE]: TunnelBookmarkServerNode,
};

export function StackTunnelsFlowEditor(props: {
  nodes: TunnelNode[];
  edges: TunnelEdge[];
  setNodes: React.Dispatch<React.SetStateAction<TunnelNode[]>>;
  setEdges: React.Dispatch<React.SetStateAction<TunnelEdge[]>>;
  onConnect: (c: { source?: string | null; target?: string | null }) => void;
  onNodeClick: (event: React.MouseEvent, node: TunnelNode) => void;
  onEdgeClick: (event: React.MouseEvent, edge: TunnelEdge) => void;
  onPaneClick: (event: React.MouseEvent) => void;
  onBeforeDelete?: OnBeforeDelete<TunnelNode, TunnelEdge>;
}) {
  const onNodesChange: OnNodesChange<TunnelNode> = (changes) => {
    props.setNodes((cur) => applyNodeChanges(changes, cur));
  };
  const onEdgesChange: OnEdgesChange<TunnelEdge> = (changes) => {
    props.setEdges((cur) => applyEdgeChanges(changes, cur));
  };

  return (
    <ReactFlowProvider>
      <div className="stack-tun-flow h-[460px] w-full overflow-hidden rounded-lg border border-border-subtle bg-black/40 [&_.react-flow__attribution]:text-slate-600">
        <ReactFlow
          nodes={props.nodes}
          edges={props.edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={props.onConnect}
          onNodeClick={props.onNodeClick}
          onEdgeClick={props.onEdgeClick}
          onPaneClick={props.onPaneClick}
          {...(props.onBeforeDelete ? { onBeforeDelete: props.onBeforeDelete } : {})}
          fitView
          colorMode="dark"
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} color="rgba(148,163,184,0.12)" />
          <Controls />
          <MiniMap
            className="!rounded-lg !border !border-white/10 !bg-black/55"
            nodeColor={(n) => {
              if (n.type === LOCAL_PC_NODE_TYPE) return "rgba(52,211,153,0.58)";
              if (n.type === BOOKMARK_SERVER_NODE_TYPE) return "rgba(251,191,36,0.55)";
              return "rgba(148,163,184,0.42)";
            }}
          />
        </ReactFlow>
      </div>
    </ReactFlowProvider>
  );
}
