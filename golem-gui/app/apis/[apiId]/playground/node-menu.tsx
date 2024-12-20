import { Menu, Transition } from "@headlessui/react";
import { Fragment } from "react";
import { CiSquareChevDown } from "react-icons/ci";
import { TrashIcon } from "lucide-react";
import useStore from "@/lib/hooks/use-react-flow-store";
import { IoMdSettings } from "react-icons/io";
import { FlowNode } from "@/types/react-flow";
import { canDelete as checkForDeletion } from "@/lib/react-flow/utils";
import EditIcon from "@mui/icons-material/Edit";
export default function NodeMenu({
  data,
  id,
  triggerType,
}: {
  data: FlowNode["data"];
  id: string;
  triggerType: string;
}) {
  const stopPropagation = (e: React.MouseEvent<HTMLButtonElement>) => {
    e.stopPropagation();
  };
  const hideMenu =
    data?.type?.includes("empty") ||
    id?.includes("end") ||
    id?.includes("start");
  const { setSelectedNode, setTrigger } = useStore();
  const canDelete = checkForDeletion(data);
  return (
    <>
      {data && !hideMenu && (
        <Menu as="div" className="relative inline-block text-left z-10">
          <div>
            <Menu.Button
              className="inline-flex w-full justify-center rounded-md text-sm"
              onClick={stopPropagation}
            >
              <CiSquareChevDown className="size-6 text-gray-500 hover:text-gray-700" />
            </Menu.Button>
          </div>
          <Transition
            as={Fragment}
            enter="transition ease-out duration-100"
            enterFrom="transform opacity-0 scale-95"
            enterTo="transform opacity-100 scale-100"
            leave="transition ease-in duration-75"
            leaveFrom="transform opacity-100 scale-100"
            leaveTo="transform opacity-0 scale-95"
          >
            <Menu.Items className="absolute right-0 w-36 origin-top-right divide-y divide-gray-100 rounded-md bg-white shadow-lg ring-1 ring-black ring-opacity-5 focus:outline-none">
              <div className="px-1 py-1">
                {triggerType === "api" && (
                  <Menu.Item>
                    {({ active }) => (
                      <button
                        onClick={(e) => {
                          stopPropagation(e);
                          setTrigger({
                            type: triggerType,
                            operation: "new_version",
                            id,
                          });

                          // deleteNodes(id);
                        }}
                        className={`${
                          active ? "bg-slate-200" : "text-gray-900"
                        } group flex w-full items-center rounded-md px-2 py-2 text-xs`}
                      >
                        <TrashIcon
                          className="mr-2 h-4 w-4"
                          aria-hidden="true"
                        />
                        New Version
                      </button>
                    )}
                  </Menu.Item>
                )}
                <Menu.Item>
                  {({ active }) => (
                    <button
                      onClick={(e) => {
                        stopPropagation(e);
                        if (!canDelete) {
                          return;
                        }
                        setTrigger({
                          type: triggerType,
                          operation: "delete",
                          id,
                        });

                        // deleteNodes(id);
                      }}
                      disabled={!canDelete}
                      className={`${
                        active ? "bg-slate-200" : "text-gray-900"
                      } group flex w-full items-center rounded-md px-2 py-2 text-xs`}
                    >
                      <TrashIcon className="mr-2 h-4 w-4" aria-hidden="true" />
                      Delete
                    </button>
                  )}
                </Menu.Item>
                <Menu.Item>
                  {({ active }) => (
                    <button
                      onClick={(e) => {
                        stopPropagation(e);
                        setTrigger({
                          type: triggerType,
                          operation: "view",
                          id,
                        });
                        setSelectedNode(id);
                      }}
                      className={`${
                        active ? "bg-slate-200" : "text-gray-900"
                      } group flex w-full items-center rounded-md px-2 py-2 text-xs`}
                    >
                      <IoMdSettings
                        className="mr-2 h-4 w-4"
                        aria-hidden="true"
                      />
                      View Details
                    </button>
                  )}
                </Menu.Item>
                <Menu.Item>
                  {({ active }) => (
                    <button
                      onClick={(e) => {
                        stopPropagation(e);
                        if (!canDelete) {
                          return;
                        }
                        setTrigger({
                          type: triggerType,
                          operation: "update",
                          id,
                        });
                        setSelectedNode(id);
                      }}
                      disabled={!canDelete}
                      className={`${
                        active ? "bg-slate-200" : "text-gray-900"
                      } group flex w-full items-center rounded-md px-2 py-2 text-xs`}
                    >
                      <EditIcon className="mr-2 h-4 w-4" aria-hidden="true" />
                      Update
                    </button>
                  )}
                </Menu.Item>
              </div>
            </Menu.Items>
          </Transition>
        </Menu>
      )}
    </>
  );
}
