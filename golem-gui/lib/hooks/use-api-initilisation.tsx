/* eslint-disable @typescript-eslint/ban-ts-comment */
/* eslint-disable @typescript-eslint/no-explicit-any */
import { useEffect, useState, useCallback } from "react";
import { Edge, useReactFlow } from "@xyflow/react";
import useStore from "./use-react-flow-store";
import dagre, { graphlib } from "@dagrejs/dagre";
import { processApiFlow } from "../react-flow/utils";
import {
  ApiStep,
  FlowNode,
} from "@/types/react-flow";
import { ApiDefinition } from "@/types/api";

const getLayoutedElements = (
  nodes: FlowNode[],
  edges: Edge[],
  options = {}
) => {
  // @ts-ignore
  const isHorizontal = options?.["elk.direction"] === "RIGHT";
  const dagreGraph = new graphlib.Graph();
  dagreGraph.setDefaultEdgeLabel(() => ({}));

  // Set graph direction and spacing
  dagreGraph.setGraph({
    rankdir: isHorizontal ? "LR" : "TB",
    nodesep: 80,
    ranksep: 80,
    edgesep: 80,
  });

  // Add nodes to dagre graph
  nodes.forEach((node) => {
    const type = node?.data?.type
      ?.replace("step-", "")
      ?.replace("action-", "")
      ?.replace("condition-", "")
      ?.replace("__end", "");

    const width = ["start", "end"].includes(type) ? 80 : 280;
    const height = ["start", "end"].includes(type) ? 80: 95;

    dagreGraph.setNode(node.id, { width, height });
  });

  // Add edges to dagre graph
  edges.forEach((edge) => {
    dagreGraph.setEdge(edge.source, edge.target);
  });

  // Run the layout
  dagre.layout(dagreGraph);

  // Get the positioned nodes and edges
  const layoutedNodes = nodes.map((node) => {
    const dagreNode = dagreGraph.node(node.id);
    return {
      ...node,
      targetPosition: isHorizontal ? "left" : "top",
      sourcePosition: isHorizontal ? "right" : "bottom",
      style: {
        ...node.style,
        width: dagreNode.width as number,
        height: dagreNode.height as number,
      },
      // Dagre provides positions with the center of the node as origin
      position: {
        x: dagreNode.x - dagreNode.width / 2,
        y: dagreNode.y - dagreNode.height / 2,
      },
    };
  });

  return {
    nodes: layoutedNodes,
    edges,
  };
};

const useApiInitialization = (
    apiDefnitions: ApiDefinition[]
) => {
  const {
    nodes,
    edges,
    setNodes,
    setEdges,
    onNodesChange,
    onEdgesChange,
    onConnect,
    onDragOver,
    onDrop,
    setV2Properties,
    openGlobalEditor,
    selectedNode,
    isLayouted,
    setIsLayouted,
    setChanges,
    setSelectedNode,
    setFirstInitilisationDone,
  } = useStore();

  const [isLoading, setIsLoading] = useState(true);
  const { screenToFlowPosition } = useReactFlow();
  const [finalNodes, setFinalNodes] = useState<FlowNode[]>([]);
  const [finalEdges, setFinalEdges] = useState<Edge[]>([]);

  const handleDrop = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      onDrop(event, screenToFlowPosition);
    },
    [screenToFlowPosition]
  );

  const onLayout = useCallback(
    ({
      direction,
      useInitialNodes = false,
      initialNodes,
      initialEdges,
    }: {
      direction: string;
      useInitialNodes?: boolean;
      initialNodes?: FlowNode[];
      initialEdges?: Edge[];
    }) => {
      const opts = { "elk.direction": direction };
      const ns = useInitialNodes ? initialNodes : nodes;
      const es = useInitialNodes ? initialEdges : edges;

      const { nodes: _layoutedNodes, edges: _layoutedEdges } =
        // @ts-expect-error
        getLayoutedElements(ns, es, opts);
      const layoutedEdges = _layoutedEdges.map((edge: Edge) => {
        return {
          ...edge,
          animated: !!edge?.target?.includes("empty"),
          data: { ...edge.data, isLayouted: true },
        };
      });
      // @ts-ignore
      const layoutedNodes = _layoutedNodes.map((node: FlowNode) => {
        return {
          ...node,
          data: { ...node.data, isLayouted: true },
        };
      });
      setNodes(layoutedNodes);
      setEdges(layoutedEdges);
      setIsLayouted(true);
      setFinalEdges(layoutedEdges);
      setFinalNodes(layoutedNodes);
    },
    [nodes, edges]
  );

  useEffect(() => {
    if (!isLayouted && nodes.length > 0) {
      onLayout({ direction: "DOWN" });
    }
  }, [nodes, edges]);

  useEffect(() => {
    const initializeWorkflow = async () => {
      setIsLoading(true);
      const sequences =  [
        ...(apiDefnitions.map((api)=>({...api, type: "api", name: api.id, isLayouted: false}))) as ApiStep[]
      ]
      const {nodes, edges} = processApiFlow(sequences, true)
      const lastestVersion = apiDefnitions[apiDefnitions.length-1];
      setSelectedNode(null);
      setFirstInitilisationDone(false);
      setIsLayouted(false);
      setNodes(nodes);
      setEdges(edges);
      setV2Properties(lastestVersion);
      setChanges(1);
      setIsLoading(false);
    };
    initializeWorkflow();
  }, []);

  return {
    nodes: finalNodes,
    edges: finalEdges,
    isLoading,
    onNodesChange: onNodesChange,
    onEdgesChange: onEdgesChange,
    onConnect: onConnect,
    onDragOver: onDragOver,
    onDrop: handleDrop,
    openGlobalEditor,
    selectedNode,
    setNodes,
    isLayouted,
  };
};

export default useApiInitialization;
