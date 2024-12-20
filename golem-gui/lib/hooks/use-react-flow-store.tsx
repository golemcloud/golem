/* eslint-disable @typescript-eslint/no-explicit-any */
import { create } from "zustand";
import { v4 as uuidv4 } from "uuid";
import {
  addEdge,
  applyNodeChanges,
  applyEdgeChanges,
  Edge,
} from "@xyflow/react";

// import { createDefaultNodeV2, createCustomEdgeMeta, processApiFlow} from "../react-flow/utils";
import { FlowNode, FlowState } from "@/types/react-flow";

export type StoreGet = () => FlowState;
export type StoreSet = (
  state:
    | FlowState
    | Partial<FlowState>
    | ((state: FlowState) => FlowState | Partial<FlowState>)
) => void;

// function addNodeBetween(
//   nodeOrEdge: string | null,
//   step: V2Step,
//   type: string,
//   set: StoreSet,
//   get: StoreGet
// ) {
//   if (!nodeOrEdge || !step) return;
//   let edge = {} as Edge;
//   if (type === "node") {
//     edge = get().edges.find((edge) => edge.target === nodeOrEdge) as Edge;
//   }

//   if (type === "edge") {
//     edge = get().edges.find((edge) => edge.id === nodeOrEdge) as Edge;
//   }

//   // eslint-disable-next-line prefer-const
//   let { source: sourceId, target: targetId } = edge || {};
//   if (!sourceId || !targetId) return;

//   const nodes = get().nodes;

//   const targetIndex = nodes.findIndex((node) => node.id === targetId);
//   const sourceIndex = nodes.findIndex((node) => node.id === sourceId);
//   if (targetIndex == -1) {
//     return;
//   }

//   if (sourceId === "trigger_start") {
//     targetId = "trigger_end";
//   }
//   const newNodeId = uuidv4();
//   const cloneStep = JSON.parse(JSON.stringify(step));
//   const newStep = { ...cloneStep, id: newNodeId };
//   const edges = get().edges;

//   // eslint-disable-next-line prefer-const
//   let { nodes: newNodes, edges: newEdges } = processApiFlow(
//     [
//       {
//         id: sourceId,
//         type: "temp_node",
//         name: "temp_node",
//         componentType: "temp_node",
//         edgeLabel: edge.label,
//         edgeColor: edge?.style?.stroke,
//       },
//       newStep,
//       {
//         id: targetId,
//         type: "temp_node",
//         name: "temp_node",
//         componentType: "temp_node",
//         edgeNotNeeded: true,
//       },
//     ] as V2Step[],
//     { x: 0, y: 0 },
//     true
//   );

//   const finalEdges = [
//     ...newEdges,
//     ...(edges.filter(
//       (edge) => !(edge.source == sourceId && edge.target == targetId)
//     ) || []),
//   ];

//   const isNested = !!(
//     nodes[targetIndex]?.isNested || nodes[sourceIndex]?.isNested
//   );
//   newNodes = newNodes.map((node) => ({ ...node, isNested }));
//   newNodes = [
//     ...nodes.slice(0, targetIndex),
//     ...newNodes,
//     ...nodes.slice(targetIndex),
//   ];
//   set({
//     edges: finalEdges,
//     nodes: newNodes,
//     isLayouted: false,
//     changes: get().changes + 1,
//   });
//   if (type == "edge") {
//     set({
//       selectedEdge: edges[edges.length - 1]?.id,
//     });
//   }

//   if (type === "node") {
//     set({ selectedNode: nodeOrEdge });
//   }

//   switch (newNodeId) {
//     case "interval":
//     case "manual": {
//       set({ v2Properties: { ...get().v2Properties, [newNodeId]: "" } });
//       break;
//     }
//     case "alert": {
//       set({ v2Properties: { ...get().v2Properties, [newNodeId]: {} } });
//       break;
//     }
//     case "incident": {
//       set({ v2Properties: { ...get().v2Properties, [newNodeId]: {} } });
//       break;
//     }
//   }
//   //on adding new node. highlight the added node and update the editor
//   set({selectedNode: newNodeId, stepEditorOpenForNode: newNodeId})
// }

const useStore = create<FlowState>((set, get) => ({
  nodes: [],
  edges: [],
  selectedNode: null,
  v2Properties: {},
  openGlobalEditor: true,
  stepEditorOpenForNode: null,
  toolboxConfiguration: {} as Record<string, any>,
  isLayouted: false,
  selectedEdge: null,
  changes: 0,
  lastSavedChanges: { nodes: [], edges: [] },
  firstInitilisationDone: false,
  errorNode: null,
  synced: true,
  canDeploy: false,
  stepErrors: null,
  globalErrors: null,
  trigger: null,
  setGlobalErros: (errors: Record<string, string> | null) =>
    set({ globalErrors: errors }),
  setStepErrors: (errors: Record<string, string> | null) =>
    set({ stepErrors: errors }),
  setCanDeploy: (deploy) => set({ canDeploy: deploy }),
  setSynced: (sync) => set({ synced: sync }),
  setErrorNode: (id) => set({ errorNode: id }),
  setTrigger: (trigger: { type: string; operation: string } | null) =>
    set({ trigger: trigger }),
  setFirstInitilisationDone: (firstInitilisationDone) =>
    set({ firstInitilisationDone }),
  setLastSavedChanges: ({
    nodes,
    edges,
  }: {
    nodes: FlowNode[];
    edges: Edge[];
  }) => set({ lastSavedChanges: { nodes, edges } }),
  setSelectedEdge: (id) =>
    set({ selectedEdge: id, selectedNode: null, openGlobalEditor: true }),
  setChanges: (changes: number) => set({ changes: changes }),
  setIsLayouted: (isLayouted) => set({ isLayouted }),
  getEdgeById: (id) => get().edges.find((edge) => edge.id === id),
  // addNodeBetween: (nodeOrEdge: string | null, step: any, type: string) => {
  //   addNodeBetween(nodeOrEdge, step, type, set, get);
  // },
  setToolBoxConfig: (config) => set({ toolboxConfiguration: config }),
  setOpneGlobalEditor: (open) => set({ openGlobalEditor: open }),
  updateSelectedNodeData: (key, value) => {
    const currentSelectedNode = get().selectedNode;
    if (currentSelectedNode) {
      const updatedNodes = get().nodes.map((node) => {
        if (node.id === currentSelectedNode) {
          //properties changes  should not reconstructed the defintion. only recontrreconstructing if there are any structural changes are done on the flow.
          if (value) {
            node.data[key] = value;
          }
          if (!value) {
            delete node.data[key];
          }
          return { ...node };
        }
        return node;
      });
      set({
        nodes: updatedNodes,
        changes: get().changes + 1,
      });
    }
  },
  setV2Properties: (properties) =>
    set({ v2Properties: properties, canDeploy: false }),
  updateV2Properties: (properties) => {
    const updatedProperties = { ...get().v2Properties, ...properties };
    set({
      v2Properties: updatedProperties,
      changes: get().changes + 1,
      canDeploy: false,
    });
  },
  setSelectedNode: (id) => {
    set({
      selectedNode: id || null,
      openGlobalEditor: false,
      selectedEdge: null,
    });
  },
  setStepEditorOpenForNode: (nodeId) => {
    set({ openGlobalEditor: false });
    set({ stepEditorOpenForNode: nodeId });
  },
  onNodesChange: (changes) =>
    set({ nodes: applyNodeChanges(changes, get().nodes) }),
  onEdgesChange: (changes) =>
    set({ edges: applyEdgeChanges(changes, get().edges) }),
  onConnect: (connection) => {
    const { source, target } = connection;
    const sourceNode = get().getNodeById(source);
    const targetNode = get().getNodeById(target);

    // Define the connection restrictions
    const canConnect = (
      sourceNode: FlowNode | undefined,
      targetNode: FlowNode | undefined
    ) => {
      if (!sourceNode || !targetNode) return false;

      // Restriction logic based on node types
      return get().edges.filter((edge) => edge.source === source).length === 0;
    };

    // Check if the connection is allowed
    if (canConnect(sourceNode, targetNode)) {
      const edge = { ...connection, type: "custom-edge" };
      set({ edges: addEdge(edge, get().edges) });
      set({
        nodes: get().nodes.map((node) => {
          if (node.id === target) {
            return { ...node, prevStepId: source, isDraggable: false };
          }
          if (node.id === source) {
            return { ...node, isDraggable: false };
          }
          return node;
        }),
      });
    } else {
      console.warn("Connection not allowed based on node types");
    }
  },

  onDragOver: (event) => {
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
  },
  onDrop: (event, screenToFlowPosition) => {
    event.preventDefault();
    event.stopPropagation();

    try {
      let step: any = event.dataTransfer.getData("application/reactflow");
      if (!step) {
        return;
      }
      step = JSON.parse(step);
      if (!step) return;
      // Use the screenToFlowPosition function to get flow coordinates
      const position = screenToFlowPosition({
        x: event.clientX,
        y: event.clientY,
      });
      const newUuid = uuidv4();
      const newNode = {
        id: newUuid,
        type: "custom",
        position, // Use the position object with x and y
        data: {
          label: step.name! as string,
          ...step,
          id: newUuid,
          name: step.name,
          type: step.type,
          componentType: step.componentType,
        },
        isDraggable: true,
        dragHandle: ".custom-drag-handle",
      } as FlowNode;

      set({ nodes: [...get().nodes, newNode] });
    } catch (err) {
      console.error(err);
    }
  },
  setNodes: (nodes) => set({ nodes }),
  setEdges: (edges) => set({ edges }),
  hasNode: (id) => !!get().nodes.find((node) => node.id === id),
  getNodeById: (id) => get().nodes.find((node) => node.id === id),
  // deleteEdges: (ids) => {
  //   const idArray = Array.isArray(ids) ? ids : [ids];
  //   set({ edges: get().edges.filter((edge) => !idArray.includes(edge.id)) });
  // },
  // deleteNodes: (ids) => {
  //   //for now handling only single node deletion. can later enhance to multiple deletions
  //   if (typeof ids !== "string") {
  //     return;
  //   }
  //   const nodes = get().nodes;
  //   const nodeStartIndex = nodes.findIndex((node) => ids == node.id);
  //   if (nodeStartIndex === -1) {
  //     return;
  //   }
  //   let idArray = Array.isArray(ids) ? ids : [ids];

  //   const startNode = nodes[nodeStartIndex];
  //   const customIdentifier = `${startNode?.data?.type}__end__${startNode?.id}`;

  //   let endIndex = nodes.findIndex((node) => node.id === customIdentifier);
  //   endIndex = endIndex === -1 ? nodeStartIndex : endIndex;

  //   const endNode = nodes[endIndex];

  //   let edges = get().edges;
  //   let finalEdges = edges;
  //   idArray = nodes.slice(nodeStartIndex, endIndex + 1).map((node) => node.id);

  //   finalEdges = edges.filter(
  //     (edge) =>
  //       !(idArray.includes(edge.source) || idArray.includes(edge.target))
  //   );
  //   if (
  //     ["interval", "alert", "manual", "incident"].includes(ids) &&
  //     edges.some(
  //       (edge) => edge.source === "trigger_start" && edge.target !== ids
  //     )
  //   ) {
  //     edges = edges.filter((edge) => !idArray.includes(edge.source));
  //   }
  //   const sources = [
  //     ...new Set(edges.filter((edge) => startNode.id === edge.target)),
  //   ];
  //   const targets = [
  //     ...new Set(edges.filter((edge) => endNode.id === edge.source)),
  //   ];
  //   targets.forEach((edge) => {
  //     const target =
  //       edge.source === "trigger_start" ? "triggger_end" : edge.target;

  //     finalEdges = [
  //       ...finalEdges,
  //       ...sources
  //         .map((source: Edge) =>
  //           createCustomEdgeMeta(source.source, target, source.label as string)
  //         )
  //         .flat(1),
  //     ];
  //   });
  //   // }

  //   nodes[endIndex + 1].position = { x: 0, y: 0 };

  //   const newNode = createDefaultNodeV2(
  //     { ...nodes[endIndex + 1].data, islayouted: false },
  //     nodes[endIndex + 1].id
  //   );

  //   const newNodes = [
  //     ...nodes.slice(0, nodeStartIndex),
  //     newNode,
  //     ...nodes.slice(endIndex + 2),
  //   ];
  //   if (["manual", "alert", "interval", "incident"].includes(ids)) {
  //     const v2Properties = get().v2Properties;
  //     delete v2Properties[ids];
  //     set({ v2Properties });
  //   }
  //   set({
  //     edges: finalEdges,
  //     nodes: newNodes,
  //     selectedNode: null,
  //     isLayouted: false,
  //     changes: get().changes + 1,
  //     openGlobalEditor: true,
  //   });
  // },
  updateEdge: (id: string, key: string, value: any) => {
    const edge = get().edges.find((e) => e.id === id);
    if (!edge) return;
    const newEdge = { ...edge, [key]: value };
    set({ edges: get().edges.map((e) => (e.id === edge.id ? newEdge : e)) });
  },
  updateNode: (node) =>
    set({ nodes: get().nodes.map((n) => (n.id === node.id ? node : n)) }),
  duplicateNode: (node) => {
    const { data, position } = node;
    const newUuid = uuidv4();
    const newNode: FlowNode = {
      ...node,
      data: {
        ...data,
        id: newUuid,
      },
      isDraggable: true,
      id: newUuid,
      position: { x: position.x + 100, y: position.y + 100 },
      dragHandle: ".custom-drag-handle",
    };
    set({ nodes: [...get().nodes, newNode] });
  },
}));

export default useStore;
