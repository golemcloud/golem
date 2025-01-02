import {
  Box,
  Drawer,
  List,
  ListItem,
  ListItemIcon,
  ListItemText,
  Typography,
  Button,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import { PanelRightClose } from "lucide-react";
import { Home, Settings, RocketLaunch, Add } from "@mui/icons-material";

import CodeIcon from "@mui/icons-material/Code";
import ArticleIcon from "@mui/icons-material/Article";
import Link from "next/link";
import { usePathname, useParams } from "next/navigation";
import { useState } from "react";
import { Button2 } from "@/components/ui/button";
import {Dropdown} from "@/components/ui/dropdown-button";


type secondaryHeaderProps = {
  onClick: () => void;
  variant: string;
  id?: string;
};

export default function SecondaryHeader({
  onClick,
  variant,
  id,
}: secondaryHeaderProps) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const pathname = usePathname();
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();

  const navigationLinks = [
    {
      name: "Overview",
      href: `/components/${compId}/overview`,
      icon: <Home fontSize="small" />,
    },
    {
      name: "Workers",
      href: `/components/${compId}/workers`,
      icon: <CodeIcon fontSize="small" />,
    },
    {
      name: "Exports",
      href: `/components/${compId}/exports`,
      icon: <RocketLaunch fontSize="small" />,
    },
    {
      name: "Files",
      href: `/components/${compId}/files`,
      icon: <ArticleIcon fontSize="small" />,
    },
    {
      name: "Settings",
      href: `/components/${compId}/settings`,
      icon: <Settings fontSize="small" />,
    },
  ];
  const workloads = [
    { route: `/components/${compId}/settings?activeTab=1`, value: "info"},
    { route: `/components/${compId}/settings?activeTab=2`, value: "update" },
  ];
  const toggleDrawer = (open: boolean) => () => {
    setDrawerOpen(open);
  };

  return (
    <Box className="dark:bg-[#0a0a0a] border-b p-2 pr-20 ">
      <Box className="flex items-center justify-between w-full">
        <Box sx={{ display: { xs: "block", md: "none" } }}>
          <Button
            startIcon={<PanelRightClose />}
            onClick={toggleDrawer(true)}
            className="dark:text-white text-9xl ml-2"
          ></Button>
        </Box>

        {pathname === `/components/${compId}/overview` && (
          <Box sx={{ marginLeft: "auto" ,display:"flex",gap:2 }}>
            <Button2
              variant="primary"
              startIcon={<AddIcon />}
              size="md"
              onClick={onClick}
            >
              New
            </Button2>
            
            <Box className="py-1 px-2 border rounded-md hover:bg-[#222] cursor-pointer">
              {Dropdown(workloads)}
            </Box>
          </Box>
        )}

     {pathname === `/components/${compId}/workers/${workerName}` && (
         <Typography
            variant="h6"
            sx={{ fontWeight: "bold" }}
            className="mx-auto text-gray-700 dark:text-gray-300"
          >
            {workerName}
          </Typography>
        )}   
      </Box>

      <Drawer
        anchor="left"
        open={drawerOpen}
        onClose={toggleDrawer(false)}
        PaperProps={{
          className:
            "dark:bg-[#0a0a0a] bg-white p-4 border-r border-gray-300 dark:border-[#3f3f3f]",
          sx: {
            width: 250,
          },
        }}
      >
        {variant == "apis" && (
          <Typography
            variant="subtitle2"
            sx={{
              fontWeight: "bold",
              color: "#AAAAAA",
              fontSize: "14px",
            }}
          >
            API
          </Typography>
        )}
        <List>
          {navigationLinks.map((link) => {
            const isActive =
              pathname === link.href ||
              (link.href !== "/" && pathname.startsWith(link.href));
            return (
              <Link
                key={link.name}
                href={link.href}
                style={{ textDecoration: "none", color: "inherit" }}
              >
                <ListItem
                  sx={{
                    marginBottom: "0.8rem",
                    cursor: "pointer",
                    borderRadius: "10px",
                  }}
                  className={`dark:hover:bg-[#373737] hover:bg-[#C0C0C0]
                ${isActive ? "dark:bg-[#373737] bg-[#C0C0C0]" : "transparent"}
                `}
                >
                  <ListItemIcon sx={{ minWidth: 32, color: "inherit" }}>
                    {link.icon}
                  </ListItemIcon>
                  <ListItemText primary={link.name} />
                </ListItem>
              </Link>
            );
          })}
        </List>
        {variant == "apis" && (
          <Typography
            variant="subtitle2"
            sx={{
              fontWeight: "bold",
              color: "#AAAAAA",
              marginTop: 3,
              marginBottom: 1,
              fontSize: "14px",
            }}
          >
            Routes
          </Typography>
        )}

        {variant === "apis" && (
          <Link href={`/apis/${id}/new-route`}>
            <Button
              variant="outlined"
              sx={{
                textTransform: "none",
                padding: "6px 12px",
                fontSize: "16px",
                fontWeight: "500",
              }}
              fullWidth
              className="border  border-black dark:border-white text-black dark:text-white dark:hover:bg-[#373737] hover:bg-[#C0C0C0]"
            >
              Add
              <Add className="ml-2" />
            </Button>
          </Link>
        )}
      </Drawer>
    </Box>
  );
}
