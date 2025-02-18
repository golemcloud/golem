/* eslint-disable @typescript-eslint/no-explicit-any */
import { Edge } from "@xyflow/react";
import {
  ApiStep,
  BasicStep,
  FlowNode,
  NodeData,
  RouteStep,
  V2Step,
} from "@lib/types/react-flow";
import { ApiDefinition } from "@lib/types/api";
import ApiIcon from "@mui/icons-material/Api";
import RouteIcon from "@mui/icons-material/Route";

export const createDefaultNodeV2 = (
  step: V2Step | NodeData,
  nodeId: string,
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
  const firstApiDefintion = apiDefinitons[0];

  if (isFirstRender) {
    const startStep = {
      type: "start",
      id: "start",
      name: "start",
      isLayouted: false,
      notClickable: true,
    } as BasicStep;
    newNodes = [
      createDefaultNodeV2(startStep, "start"),
      createDefaultNodeV2(
        {
          type: "api_start",
          id: firstApiDefintion.id,
          name: firstApiDefintion.id,
          isLayouted: false,
          notClickable: true,
        } as BasicStep,
        firstApiDefintion.id
      ),
    ];
    newEdges = [
      ...createCustomEdgeMeta("_", "start"),
      ...createCustomEdgeMeta("start", firstApiDefintion.id)
    ];
  }
  apiDefinitons?.forEach((apiDefiniton: ApiStep) => {
    const { routes, ...nodeData } = apiDefiniton;
    const nodeId = `${apiDefiniton.id}__${apiDefiniton.version}__api`;
    newEdges = [
      ...newEdges, 
      ...createCustomEdgeMeta(firstApiDefintion.id, nodeId)]
    newNodes.push(createDefaultNodeV2(apiDefiniton, nodeId));
    routes.forEach((route)=>{
      const id = `${nodeId}__${route?.path}__${route?.method}__route`;
      const routeData = {
        apiInfo: { ...nodeData } as Omit<ApiDefinition, "routes">,
        ...route,
        name: route.path,
        type: "route",
      } as RouteStep;
      newNodes.push(createDefaultNodeV2(routeData, id));
      newEdges = [
        ...newEdges, 
        ...createCustomEdgeMeta(nodeId, id)]
    })

    if (routes.length === 0) {
      const emptyRoute = getRouteEmptyNode(nodeId);
      newNodes.push(createDefaultNodeV2({...emptyRoute, apiInfo:{...nodeData}, id: nodeId}, emptyRoute.id));
      newEdges = [
        ...newEdges, 
        ...createCustomEdgeMeta(nodeId, emptyRoute.id)]

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