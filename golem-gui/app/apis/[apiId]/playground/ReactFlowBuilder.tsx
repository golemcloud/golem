/* eslint-disable @typescript-eslint/no-explicit-any */
import React from "react";
import {
  ReactFlow,
  Background,
  Controls,
  EdgeTypes as EdgeTypesType,
} from "@xyflow/react";
import CustomNode from "./CustomNode";
import CustomEdge from "./CustomEdge";
import useApiInitialization from "@/lib/hooks/use-api-initilisation";
import "@xyflow/react/dist/style.css";
import { ApiDefinition } from "@/types/api";
import Editors from "./editors";
import { Paper } from "@mui/material";

const nodeTypes = { custom: CustomNode as any };
const edgeTypes: EdgeTypesType = {
  "custom-edge": CustomEdge as React.ComponentType<any>,
};

const ReactApiFlowBuilder = ({
  apiDefnitions,
}: {
  apiDefnitions: ApiDefinition[];
}) => {
  const {
    nodes,
    edges,
    isLoading,
    onEdgesChange,
    onNodesChange,
    onConnect,
    onDragOver,
    onDrop,
  } = useApiInitialization(apiDefnitions);

  return (
    <Paper
    elevation={3}
    sx={{
      p: 3,
      mb: 3,
      color: "text.primary",
      border: 1,
      borderColor: "divider",
      borderRadius: 2,
    }}
     style={{ height: "100vh", width: "100%", margin: "0 auto" }}>
      <>
      {!isLoading && (
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onDrop={onDrop}
          onDragOver={onDragOver}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          fitView
        >
          <Controls orientation="horizontal" position="top-left" className="text-black"/>
          <Background />
        </ReactFlow>
      )}
      <Editors />
      </>
    </Paper>
  );
};

export default ReactApiFlowBuilder;
