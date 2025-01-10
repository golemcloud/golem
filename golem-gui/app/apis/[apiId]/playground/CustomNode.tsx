import React, { memo } from "react";
import { Handle, Position } from "@xyflow/react";
import NodeMenu from "./node-menu";
import useStore from "@/lib/hooks/use-react-flow-store";
import { GoPlus } from "react-icons/go";
import { MdNotStarted } from "react-icons/md";
import { GoSquareFill } from "react-icons/go";
import { BiSolidError } from "react-icons/bi";
import { toast } from "react-toastify";
import { FlowNode } from "@/types/react-flow";
import {
  getIconBasedOnType,
  getStatus,
  getVersion,
  getTriggerType,
} from "@/lib/react-flow/utils";
import DoneIcon from "@mui/icons-material/Done";
import { Box, Paper, Stack, Typography, useTheme } from "@mui/material";
import { ApiRoute } from "@/types/api";

function CustomNode({ id, data }: FlowNode) {
  const { selectedNode, setSelectedNode, errorNode, synced, setTrigger } =
    useStore();
  const theme = useTheme();
  const type = data?.type;
  const isEmptyNode = !!data?.type?.includes("empty");
  const specialNodeCheck = ["start", "end"].includes(type);
  const Icon = getIconBasedOnType(data);
  const triggerType = getTriggerType(id);
  const {apiInfo, ...route} = data;

  const status = getStatus(triggerType === "api" ? data: apiInfo || {});

  function handleNodeClick(e: React.MouseEvent<HTMLDivElement>) {
    e.stopPropagation();
    if (!synced) {
      toast(
        "Please save the previous step or wait while properties sync with the workflow."
      );
      return;
    }
    if (data?.notClickable) {
      return;
    }
    if (specialNodeCheck || id?.includes("end") || id?.includes("empty")) {
      if (id?.includes("empty")) {
        setSelectedNode(id);
      }
      return;
    }
    setSelectedNode(id);
  }

  return (
    <Paper
      elevation={3}
      color="transparent"
      className="dark:bg-[#0a0a0a] bg-slate-50 dark:text-white"
      sx={{
        border: `2px solid ${
          id === selectedNode || isEmptyNode
            ? theme.palette.primary.main
            : theme.palette.divider
        }`,
        opacity: data.isLayouted ? 1 : 0.7,
        borderStyle: isEmptyNode ? "dashed" : "solid",
        width: "100%",
        height: "100%",
        borderRadius: type === "start" ? "50%" : 2,
      }}
      onClick={handleNodeClick}
    >
      {!specialNodeCheck && (
        <Box
          display="flex"
          flexDirection="column"
          justifyContent="center"
          sx={{ height: "100%" }}
        >
          {isEmptyNode && (
            <Box
              display="flex"
              flexDirection="column"
              alignItems="center"
              justifyContent="center"
              onClick={(e) => {
                e.preventDefault();
                if(status!=="Draft"){
                  return alert("Can't perform this operation on published api");
                }
                setTrigger({
                   type: triggerType, operation: triggerType === "api" ? "new_api": "new_route",
                   meta: {
                    version: triggerType === "api" ? data.version : apiInfo?.version,
                    ...(triggerType === "route"? {route: route as ApiRoute}: {})
                   }

                });
              }}
            >
              <GoPlus size={32} style={{ marginBottom: theme.spacing(1) }} />
              <Typography variant="body2">
                {data.label || "Create New"}
              </Typography>
            </Box>
          )}
          {errorNode === id && (
            <BiSolidError
              size={24}
              color={theme.palette.error.main}
              style={{
                position: "absolute",
                right: -32,
                top: -32,
              }}
            />
          )}
          {!isEmptyNode && (
            <Box
              p={2}
              display="flex"
              flexDirection="row"
              justifyContent={"space-between"}
            >
              <Stack direction="row" gap={2} overflow="hidden">
                {Icon && <Icon className="mt-2" />}
                <Box overflow="hidden">
                  <Typography variant="h6" noWrap>
                    {data?.name}
                  </Typography>
                  <Stack
                    direction="row"
                    justifyContent="space-between"
                    alignItems="center"
                  >
                    <Typography variant="body2" noWrap>
                      {type}
                      {getVersion(data)}
                    </Typography>
                  </Stack>
                  {status && (
                    <Stack direction="row" alignItems="center" gap={1}>
                      <Typography
                        variant="caption"
                        sx={{
                          border: `1px solid`,
                          px: 1,
                        }}
                        className={status !== "Draft" ? "bg-green-600" : ""}
                      >
                        {status}
                      </Typography>
                      {status === "Draft" && <DoneIcon color="primary" />}
                    </Stack>
                  )}
                </Box>
              </Stack>
              {type !== "api_start" && (
                <NodeMenu data={data} id={id} triggerType={triggerType} />
              )}
            </Box>
          )}

          <Handle type="target" position={Position.Top} className="w-32" />
          <Handle type="source" position={Position.Bottom} className="w-32" />
        </Box>
      )}

      {specialNodeCheck && (
        <>
          {type === "start" && (
            <MdNotStarted
              style={{
                backgroundColor: theme.palette.primary.main,
                color: theme.palette.common.white,
                borderRadius: "50%",
              }}
              className="w-full h-full"
            />
          )}
          {type === "end" && (
            <GoSquareFill
              size={24}
              style={{
                backgroundColor: theme.palette.primary.main,
                color: theme.palette.common.white,
                borderRadius: "50%",
              }}
            />
          )}
          {"start" === type && (
            <Handle type="source" position={Position.Bottom} className="w-32" />
          )}
          {"end" === type && (
            <Handle type="target" position={Position.Top} className="w-32" />
          )}
        </>
      )}
    </Paper>
  );
}

export default memo(CustomNode);
