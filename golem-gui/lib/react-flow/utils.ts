/* eslint-disable @typescript-eslint/no-explicit-any */
import { Edge } from "@xyflow/react";
import {
  ApiStep,
  BasicStep,
  FlowNode,
  NodeData,
  RouteStep,
  V2Step,
} from "@/types/react-flow";
import { ApiDefinition, ApiRoute } from "@/types/api";
import ApiIcon from "@mui/icons-material/Api";
import RouteIcon from "@mui/icons-material/Route";

export const createDefaultNodeV2 = (
  step: V2Step | NodeData,
  nodeId: string,
  nextNodeId?: string | null,
  prevNodeId?: string | null,
  isNested?: boolean
): FlowNode =>
  ({
    id: nodeId,
    type: "custom",
    dragHandle: ".custom-drag-handle",
    position: { x: 0, y: 0 },
    data: {
      label: step.name,
      ...step,
    },
    isDraggable: false,
    nextNodeId,
    prevNodeId,
    isNested: !!isNested,
  } as FlowNode);

const getRandomColor = () => {
  const letters = "0123456789ABCDEF";
  let color = "#";
  for (let i = 0; i < 6; i++) {
    color += letters[Math.floor(Math.random() * 16)];
  }
  return color;
};

export function createCustomEdgeMeta(
  source: string | string[],
  target: string | string[],
  label?: string,
  color?: string,
  type?: string
) {
  const finalSource = (
    Array.isArray(source) ? source : source ? [source] : []
  ) as string[];
  const finalTarget = (
    Array.isArray(target) ? target : target ? [target] : []
  ) as string[];

  const edges = [] as Edge[];
  finalSource?.forEach((source) => {
    finalTarget?.forEach((target) => {
      edges.push({
        id: `e${source}-${target}`,
        source: source ?? "",
        target: target ?? "",
        type: type || "custom-edge",
        label,
        style: { stroke: color || getRandomColor() },
      } as Edge);
    });
  });

  if (finalTarget.length === 0) {
  }

  return edges;
}
export function handleDefaultNode(
  step: V2Step,
  nextNodeId: string,
  prevNodeId: string,
  nodeId: string,
  isNested?: boolean
) {
  const nodes = [];
  let edges = [] as Edge[];
  const newNode = createDefaultNodeV2(
    step,
    nodeId,
    nextNodeId,
    prevNodeId,
    isNested
  );
  if (step.type !== "temp_node") {
    nodes.push(newNode);
  }
  // Handle edge for default nodes
  if (newNode.id !== "end" && !step.edgeNotNeeded) {
    edges = [
      ...edges,
      ...createCustomEdgeMeta(
        newNode.id,
        step.edgeTarget || nextNodeId,
        step.edgeLabel,
        step.edgeColor
      ),
    ];

    edges = [
      ...edges,
      ...(nextNodeId == ""
        ? createCustomEdgeMeta(
            newNode.id,
            step.edgeTarget || nextNodeId,
            step.edgeLabel,
            step.edgeColor
          )
        : []),
    ];
  }
  return { nodes, edges };
}

export const getTempNodes = (id: string) => {
  return {
    type: "temp_node",
    id,
    name: "temp_node",
    isLayouted: false,
  };
};

export const getRouteEndNode = (nodeId: string) => {
  return {
    type: "end",
    id: `${nodeId}_route_end`,
    name: "End",
    isLayouted: false,
  };
};

export const getRouteEmptyNode = (nodeId: string) => {
  return {
    type: "empty_route",
    id: `${nodeId}_empty__route`,
    name: "Create New Route",
    isLayouted: false,
    label: "Create New Route",
  };
};

export const processApiFlow = (
  apiDefinitons: ApiStep[],
  isFirstRender = false
) => {
  let newNodes: FlowNode[] = [];
  let newEdges: Edge[] = [];
  if (isFirstRender) {
    const startStep = {
      type: "start",
      id: "start",
      name: "start",
      isLayouted: false,
      notClickable: true,
    } as BasicStep;
    const firstApiDefintion = apiDefinitons[0];
    const { nodes, edges } = handleDefaultNode(
      startStep,
      firstApiDefintion.id,
      "",
      "start"
    );
    newNodes = [
      ...newNodes,
      ...nodes,
      createDefaultNodeV2(
        {
          type: "api_start",
          id: firstApiDefintion.id,
          name: firstApiDefintion.id,
          isLayouted: false,
          edgeNotNeeded: true,
          notClickable: true,
        } as BasicStep,
        firstApiDefintion.id
      ),
    ];
    newEdges = [...newEdges, ...edges];
  }
  const firstApiDefintion = apiDefinitons[0];
  apiDefinitons?.forEach((apiDefiniton: ApiStep) => {
    const { routes, ...nodeData } = apiDefiniton;
    const nodeId = `${apiDefiniton.id}__${apiDefiniton.version}__api`;
    const nextNodeId = routes[0]
      ? `${nodeId}__${routes[0]?.path}__${routes[0]?.method}__route`
      : "";
    const { nodes, edges } = handleDefaultNode(
      apiDefiniton,
      nextNodeId,
      firstApiDefintion.id,
      nodeId
    );

    console.log("apiDefiniton.id", apiDefiniton.id);
    console.log("nodes, edges", nodes, edges);

    newNodes = [...newNodes, ...nodes];
    newEdges = [
      ...newEdges,
      ...edges,
      ...createCustomEdgeMeta(firstApiDefintion.id, nodeId),
    ];
    routes.forEach((route: ApiRoute) => {
      const routeData = {
        apiInfo: { ...nodeData } as Omit<ApiDefinition, "routes">,
        ...route,
        name: route.path,
        type: "route"
      } as RouteStep;
      const routeId = `${nodeId}__${route?.path}__${route?.method}__route`;
      const tempNodes = [
        getTempNodes(nodeId),
        routeData,
        { ...getRouteEndNode(nodeId), type: "temp_node", edgeNotNeeded: true },
      ];

      tempNodes.forEach((node, index) => {
        const previousId = tempNodes[index - 1]?.id;
        const nextId = tempNodes[index + 1]?.id || "";
        const id = index === 1 ? routeId : node.id;
        const { nodes, edges } = handleDefaultNode(
          node,
          nextId,
          previousId,
          id
        );
        newNodes = [...newNodes, ...nodes];
        newEdges = [...newEdges, ...edges];
      });
    });

    if (routes.length === 0) {
      const tempNodes = [
        getTempNodes(nodeId),
        getRouteEmptyNode(nodeId),
        { ...getRouteEndNode(nodeId), type: "temp_node", edgeNotNeeded: true },
      ];
      tempNodes.forEach((node, index) => {
        const previousId = index === 0 ? nodeId : tempNodes[index - 1]?.id;
        const nextId = tempNodes[index + 1]?.id || "";
        const { nodes, edges } = handleDefaultNode(
          node,
          nextId,
          previousId,
          node.id
        );
        newNodes = [...newNodes, ...nodes];
        newEdges = [...newEdges, ...edges];
      });
    }
  });
  if (isFirstRender) {
    newNodes = newNodes.map((node) => ({ ...node, isLayouted: false }));
    newEdges = newEdges.map((edge) => ({ ...edge, isLayouted: false }));
  }
  return { nodes: newNodes, edges: newEdges };
};


export function getIconBasedOnType(data: FlowNode["data"]) {
  switch (data.type) {
    case "api":
    case "api_start":
      return ApiIcon;
    case "route":
      return RouteIcon;
    default:
      return null;
  }
}


export function getVersion(data: FlowNode["data"]) {
  switch(data.type) {
    case "api": return `(${data.version})`
    default: return ""
  }

}
export function getStatus(data: FlowNode["data"]) {
  switch(data.type) {
    case "api": return data.draft ? "Draft" : "Published";
    default: return ""
  }

}

export function canDelete(data: FlowNode["data"]) {
  switch(data.type) {
    case "api": return data.draft || false
    case "route": return data?.apiInfo?.draft || false;
    default: return false;
  }
}

export function getTriggerType(id: string) {
  const meta = id.split("__");
  return meta[meta.length - 1] || "";
}