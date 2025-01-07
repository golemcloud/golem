import * as React from "react";
import ClickAwayListener from "@mui/material/ClickAwayListener";
import Grow from "@mui/material/Grow";
import Paper from "@mui/material/Paper";
import Popper from "@mui/material/Popper";
import MenuItem from "@mui/material/MenuItem";
import MenuList from "@mui/material/MenuList";
import { Box } from "@mui/material";
import { CiSquareChevDown } from "react-icons/ci";
import { Button2 as Button } from "@/components/ui/button";
import { canDelete as checkForDeletion } from "@/lib/react-flow/utils";
import { FlowNode } from "@/types/react-flow";
import useStore from "@/lib/hooks/use-react-flow-store";

export default function NodeMenu({
  data,
  id,
  triggerType,
}: {
  data: FlowNode["data"];
  id: string;
  triggerType: string;
}) {
  // const stopPropagation = (e: React.MouseEvent<HTMLButtonElement>) => {
  //   e.stopPropagation();
  // };
  const { setSelectedNode, setTrigger } = useStore();
  const canDelete = checkForDeletion(data);
  const [open, setOpen] = React.useState(false);
  const anchorRef = React.useRef<HTMLButtonElement>(null);

  const handleToggle = () => {
    setOpen((prevOpen) => !prevOpen);
  };

  const handleClose = (event: Event | React.SyntheticEvent) => {
    if (
      anchorRef.current &&
      anchorRef.current.contains(event.target as HTMLElement)
    ) {
      return;
    }

    setOpen(false);
  };

  function handleListKeyDown(event: React.KeyboardEvent) {
    if (event.key === "Tab") {
      event.preventDefault();
      setOpen(false);
    } else if (event.key === "Escape") {
      setOpen(false);
    }
  }

  // return focus to the button when we transitioned from !open -> open
  const prevOpen = React.useRef(open);
  React.useEffect(() => {
    if (prevOpen.current === true && open === false) {
      anchorRef.current!.focus();
    }

    prevOpen.current = open;
  }, [open]);

  return (
    <Box className="dark:bg-[#0a0a0a] bg-white dark:text-white p-0 relative">
        <Button
          ref={anchorRef}
          id="composition-button"
          aria-controls={open ? "composition-menu" : undefined}
          aria-expanded={open ? "true" : undefined}
          aria-haspopup="true"
          onClick={handleToggle}
          variant={"ghost"}
        >
          <CiSquareChevDown className="size-24 text-gray-500 hover:text-gray-700" />
        </Button>
        <Popper
          open={open}
          anchorEl={anchorRef.current}
          role={undefined}
          placement="bottom-start"
          transition
          disablePortal={false}
          style={{ zIndex: 20 }}
        >
          {({ TransitionProps, placement }) => (
            <Grow
              {...TransitionProps}
              style={{
                transformOrigin:
                  placement === "bottom-start" ? "left top" : "left bottom",
              }}
            >
              <Paper
                className="dark:bg-[#0c0c0c] bg-slate-50 border border-gray-300 dark:border-[#3f3f3f] dark:text-white border-solid"
              >
                <ClickAwayListener onClickAway={handleClose}>
                  <MenuList
                    autoFocusItem={open}
                    id="composition-menu"
                    aria-labelledby="composition-button"
                    onKeyDown={handleListKeyDown}
                  >
                     {triggerType !== "route" && <MenuItem
                      disabled={!canDelete}
                      onClick={(e) => {
                        setTrigger({
                          type: triggerType,
                          operation: "new_route",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        handleClose(e);
                      }}
                    >
                      New Route
                    </MenuItem>}
                    {triggerType !== "route" &&  <MenuItem
                      onClick={(e) => {
                        setTrigger({
                          type: triggerType,
                          operation: "create",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        handleClose(e);
                      }}
                    >
                      New Version
                    </MenuItem>}
                    <MenuItem
                      disabled={!canDelete}
                      onClick={(e) => {
                        if (!canDelete) {
                          return handleClose(e);
                        }
                        setTrigger({
                          type: triggerType,
                          operation: "delete",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        handleClose(e);
                      }}
                    >
                      Delete
                    </MenuItem>
                    <MenuItem
                      onClick={(e) => {
                        setTrigger({
                          type: triggerType,
                          operation: "view",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        setSelectedNode(id);
                        handleClose(e);
                      }}
                    >
                      View Details
                    </MenuItem>
                    {triggerType !== "api" && <MenuItem
                      onClick={(e) => {
                        if (!canDelete) {
                          return handleClose(e);
                        }
                        setTrigger({
                          type: triggerType,
                          operation: "update",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        setSelectedNode(id);
                        handleClose(e);
                      }}
                      disabled={!canDelete}
                    >
                      Update
                    </MenuItem>}
                    {triggerType !== "route" && <MenuItem
                      onClick={(e) => {
                        if (!canDelete) {
                          return handleClose(e);
                        }
                        setTrigger({
                          type: triggerType,
                          operation: "download",
                          id,
                          meta: {version: triggerType=="api" ? data.version: data?.apiInfo?.version}
                        });
                        setSelectedNode(id);
                        handleClose(e);
                      }}
                    >
                      Download
                    </MenuItem>}
                  </MenuList>
                </ClickAwayListener>
              </Paper>
            </Grow>
          )}
        </Popper>
    </Box>
  );
}
