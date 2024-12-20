import React, { memo } from "react";
import { Handle, Position } from "@xyflow/react";
import NodeMenu from "./node-menu";
import useStore from "@/lib/hooks/use-react-flow-store";
import { GoPlus } from "react-icons/go";
import { MdNotStarted } from "react-icons/md";
import { GoSquareFill } from "react-icons/go";
import { BiSolidError } from "react-icons/bi";
import { toast } from "react-toastify";
import { FlowNode, Trigger } from "@/types/react-flow";
import {
  getIconBasedOnType,
  getStatus,
  getVersion,
  getTriggerType,
} from "@/lib/react-flow/utils";
import DoneIcon from "@mui/icons-material/Done";
import { Stack } from "@mui/material";

function CustomNode({ id, data }: FlowNode) {
  const {
    selectedNode,
    setSelectedNode,
    setOpneGlobalEditor,
    errorNode,
    synced,
    setTrigger,
  } = useStore();
  const type = data?.type;
  const isEmptyNode = !!data?.type?.includes("empty");
  const specialNodeCheck = ["start", "end"].includes(type);
  const Icon = getIconBasedOnType(data);
  const draft = getStatus(data);
  const triggerType = getTriggerType(id);

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
      setOpneGlobalEditor(true);
      return;
    }
    setSelectedNode(id);
  }

  return (
    <>
      {!specialNodeCheck && (
        <div
          className={`flex shadow-md rounded-md bg-white border-2 w-full h-full ${
            id === selectedNode ? "border-orange-500" : "border-stone-400"
          }`}
          onClick={handleNodeClick}
          style={{
            opacity: data.isLayouted ? 1 : 0,
            borderStyle: isEmptyNode ? "dashed" : "",
            borderColor: errorNode == id ? "red" : "",
          }}
        >
          {isEmptyNode && (
            <div
              className="p-2 flex-1 flex flex-col items-center justify-center"
              onClick={(e) => {
                e.preventDefault();
                setTrigger({ type: triggerType, operation: "create" });
              }}
            >
              <GoPlus className="w-8 h-8 text-gray-600 font-bold p-0" />
              {selectedNode === id && (
                <div className="text-gray-600 font-bold text-center">
                  {data.label || "Create New"}
                </div>
              )}
            </div>
          )}
          {errorNode === id && (
            <BiSolidError className="size-16  text-red-500 absolute right-[-40px] top-[-40px]" />
          )}
          {!isEmptyNode && (
            <div className="container p-2 flex-1 flex flex-row items-start justify-between gap-2 flex-wrap">
              {Icon && <Icon />}
              {/*TODO: Refactor this with valid styles*/}
              <div className="flex-1 flex-col flex-wrap truncate">
                <div className="text-lg font-bold truncate">{data?.name}</div>
                <Stack
                  direction={"row"}
                  justifyContent={"space-between"}
                  alignItems={"center"}
                >
                  <div className="text-gray-500 truncate">
                    {type}
                    {getVersion(data)}
                  </div>
                </Stack>
                {draft && (
                  <Stack direction={"row"} alignItems={"center"}>
                    <span className="border text-sm border-black px-2  ">
                      {draft}
                    </span>
                    {draft === "Draft" && <DoneIcon />}
                  </Stack>
                )}
              </div>
              <div>
                {type !== "api_start" && (
                  <NodeMenu data={data} id={id} triggerType={triggerType} />
                )}
              </div>
            </div>
          )}

          <Handle type="target" position={Position.Top} className="w-32" />
          <Handle type="source" position={Position.Bottom} className="w-32" />
        </div>
      )}

      {specialNodeCheck && (
        <div
          style={{
            opacity: data.isLayouted ? 1 : 0,
          }}
          onClick={(e) => {
            e.stopPropagation();
            if (!synced) {
              toast(
                "Please save the previous step or wait while properties sync with the workflow."
              );
              return;
            }
            if (specialNodeCheck || id?.includes("end")) {
              setOpneGlobalEditor(true);
              return;
            }
            setSelectedNode(id);
          }}
        >
          <div className={`flex flex-col items-center justify-center`}>
            {type === "start" && (
              <MdNotStarted className="size-20 bg-orange-500 text-white rounded-full font-bold mb-2" />
            )}
            {type === "end" && (
              <GoSquareFill className="size-20 bg-orange-500 text-white rounded-full font-bold mb-2" />
            )}
            {"start" === type && (
              <Handle
                type="source"
                position={Position.Bottom}
                className="w-32"
              />
            )}

            {"end" === type && (
              <Handle type="target" position={Position.Top} className="w-32" />
            )}
          </div>
        </div>
      )}
    </>
  );
}

export default memo(CustomNode);
