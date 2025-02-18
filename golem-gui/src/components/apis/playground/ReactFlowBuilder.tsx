/* eslint-disable @typescript-eslint/no-explicit-any */
import React, { useMemo, useState } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  EdgeTypes as EdgeTypesType,
} from "@xyflow/react";
import CustomNode from "./CustomNode";
import CustomEdge from "./CustomEdge";
import useApiInitialization from "@lib/hooks/use-api-initilisation";
import "@xyflow/react/dist/style.css";
import { ApiDefinition } from "@lib/types/api";
import Editors from "./editors";
import { Box } from "@mui/material";
import { DropdownV2 } from "@ui/dropdown-button";

const nodeTypes = { custom: CustomNode as any };
const edgeTypes: EdgeTypesType = {
  "custom-edge": CustomEdge as React.ComponentType<any>,
};

const ReactApiFlowBuilder = ({
  apiDefnitions,
}: {
  apiDefnitions: ApiDefinition[];
}) => {
  const [show, setShow] = useState("All");

  const finalApiDefintions = useMemo(() => {
    return apiDefnitions?.filter((api: ApiDefinition) => {
      const status = api.draft ? "Draft" : "Published";
      return show === "All" || show === status;
    });
  }, [show, apiDefnitions]);

  const {
    nodes,
    edges,
    isLoading,
    onEdgesChange,
    onNodesChange,
    onConnect,
    onDragOver,
    onDrop,
  } = useApiInitialization(finalApiDefintions);

  const isPublished = useMemo(
    () => !!apiDefnitions?.find((api) => api.draft !== true),
    [apiDefnitions]
  );
  const isDraftFound = useMemo(
    () => !!apiDefnitions?.find((api) => api.draft == true),
    [apiDefnitions]
  );

  return (
    <Box
      className="p-3 mb-3 relative"
      style={{ height: "100vh", width: "100%", margin: "0 auto" }}
    >
      <>
        <Box position="absolute" padding={1} marginLeft={20} zIndex={100}>
          <DropdownV2
            list={[
              { label: "All", value: "All", onClick: () => setShow("All") },
              {
                label: "Published Only",
                value: "Published",
                onClick: () => {
                  if (isPublished) {
                    setShow("Published");
                  }
                },
                disabled: !isPublished,
              },
              {
                label: "Draft Only",
                value: "Draft",
                onClick: () => setShow("Draft"),
                disabled: !isDraftFound,
              },
            ]}
            prefix={show}
          />
        </Box>
        {!isLoading && finalApiDefintions.length ? (
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
            <Controls
              orientation="horizontal"
              position="top-left"
              className="text-black"
            />
            <Background />
          </ReactFlow>
        ) : null}
        <Editors />
      </>
    </Box>
  );
};

export default ReactApiFlowBuilder;
