import React, { memo } from "react";
import { Handle, Position } from "@xyflow/react";
// import NodeMenu from "./node-menu";
import useStore from "@/lib/hooks/use-react-flow-store";
import { GoPlus } from "react-icons/go";
import { MdNotStarted } from "react-icons/md";
import { GoSquareFill } from "react-icons/go";
import { BiSolidError } from "react-icons/bi";
import { toast } from "react-toastify";
import { FlowNode } from "@/types/react-flow";
import ApiIcon from "@mui/icons-material/Api";
import RouteIcon from "@mui/icons-material/Route";

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
  function getIconBasedOnType(data: FlowNode["data"]) {
    switch (data.type) {
      case "api":
      case "api_start":
        return <ApiIcon />;
      case "route":
        return <RouteIcon />;
      default:
        return null;
    }
  }

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

  console.log("enteirgnt his node", specialNodeCheck);

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
                const meta = id.split("__");
                const type = meta[meta.length - 1];
                setTrigger({ type: type, operation: "creation" });
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
            <div className="container p-2 flex-1 flex flex-row items-center justify-between gap-2 flex-wrap">
              {getIconBasedOnType(data)}
              <div className="flex-1 flex-col gap-2 flex-wrap truncate">
                <div className="text-lg font-bold truncate">{data?.name}</div>
                <div className="text-gray-500 truncate">{type}</div>
              </div>
              {/* <div>
                <NodeMenu data={data} id={id} />
              </div> */}
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
