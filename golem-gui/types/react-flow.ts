/* eslint-disable @typescript-eslint/no-explicit-any */
import {
  Edge,
  Node,
  OnConnect,
  OnEdgesChange,
  OnNodesChange,
} from "@xyflow/react";
import { ApiDefinition, ApiRoute } from "./api";

export interface LogEntry {
  timestamp: string;
  message: string;
  context: string;
}

export interface WorkflowExecution {
  id: string;
  workflow_id: string;
  tenant_id: string;
  started: string;
  triggered_by: string;
  status: string;
  results: Record<string, any>;
  workflow_name?: string;
  logs?: LogEntry[] | null;
  error?: string | null;
  execution_time?: number;
}
export type WorkflowExecutionFailure = Pick<WorkflowExecution, "error">;

export type V2Properties = Record<string, any>;
export type Definition = {
  sequence: V2Step[];
  properties: V2Properties;
  isValid?: boolean;
};
export type ReactFlowDefinition = {
  value: {
    sequence: V2Step[];
    properties: V2Properties;
  };
  isValid?: boolean;
};

export type BasicStep = {
  id: string;
  name: string;
  type: string;
  isLayouted?: boolean;
  edgeNotNeeded?: boolean;
  edgeLabel?: string;
  edgeColor?: string;
  edgeSource?: string;
  edgeTarget?: string;
  notClickable?: boolean;
  nodeId?: string;
};

export type Trigger = {
  id?:string;
  type: string;
  operation: string;
  meta?: {version: string}
} | null


export type ApiStep = ApiDefinition & BasicStep;
export type RouteStep = ApiRoute &
  BasicStep & { apiInfo: Omit<ApiDefinition, "routes"> };

export type V2Step = BasicStep | ApiStep | RouteStep;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type NodeData = Node["data"] & Record<string, any>;
export type NodeStepMeta = { id: string; label?: string };
export type FlowNode = Node & {
  prevStepId?: string | string[];
  edge_label?: string;
  data: NodeData;
  isDraggable?: boolean;
  id: string;
  isNested: boolean;
};
export type FlowState = {
  nodes: FlowNode[];
  edges: Edge[];
  selectedNode: string | null;
  trigger: Trigger;
  setTrigger: (trigger: Trigger) => void;
  onNodesChange: OnNodesChange<FlowNode>;
  onEdgesChange: OnEdgesChange<Edge>;
  onConnect: OnConnect;
  onDragOver: (event: React.DragEvent) => void;
  onDrop: (
    event: React.DragEvent,
    screenToFlowPosition: (coords: { x: number; y: number }) => {
      x: number;
      y: number;
    }
  ) => void;
  setNodes: (nodes: FlowNode[]) => void;
  setEdges: (edges: Edge[]) => void;
  getNodeById: (id: string | null) => FlowNode | undefined;
  hasNode: (id: string) => boolean;
  // deleteEdges: (ids: string | string[]) => void;
  // deleteNodes: (ids: string | string[]) => void;
  updateNode: (node: FlowNode) => void;
  duplicateNode: (node: FlowNode) => void;
  // addNode: (node: Partial<FlowNode>) => void;
  setSelectedNode: (id: string | null) => void;
  // updateNodeData: (nodeId: string, key: string, value: any) => void;
  updateSelectedNodeData: (key: string, value: any) => void;
  updateEdge: (id: string, key: string, value: any) => void;
  // addNodeBetween: (
  //   nodeOrEdge: string | null,
  //   step: V2Step,
  //   type: string
  // ) => void;
  isLayouted: boolean;
  setIsLayouted: (isLayouted: boolean) => void;
  selectedEdge: string | null;
  setSelectedEdge: (id: string | null) => void;
  getEdgeById: (id: string) => Edge | undefined;
  changes: number;
  setChanges: (changes: number) => void;
  firstInitilisationDone: boolean;
  setFirstInitilisationDone: (firstInitilisationDone: boolean) => void;
  lastSavedChanges: { nodes: FlowNode[] | null; edges: Edge[] | null };
  setLastSavedChanges: ({
    nodes,
    edges,
  }: {
    nodes: FlowNode[];
    edges: Edge[];
  }) => void;
  setErrorNode: (id: string | null) => void;
  errorNode: string | null;
  synced: boolean;
  setSynced: (synced: boolean) => void;
  canDeploy: boolean;
  setCanDeploy: (deploy: boolean) => void;
  stepErrors: Record<string, string> | null;
  setStepErrors: (errors: Record<string, string> | null) => void;
  globalErrors: Record<string, string> | null;
  setGlobalErros: (errors: Record<string, string> | null) => void;
};
export type StoreGet = () => FlowState;
export type StoreSet = (
  state:
    | FlowState
    | Partial<FlowState>
    | ((state: FlowState) => FlowState | Partial<FlowState>)
) => void;
