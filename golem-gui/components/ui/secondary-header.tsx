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
import {
  usePathname,
  useParams,
  useSearchParams,
  useRouter,
} from "next/navigation";
import { useEffect, useState } from "react";
import { Button2 } from "@/components/ui/button";
import { Dropdown } from "@/components/ui/dropdown-button";
import PlayForWorkIcon from "@mui/icons-material/PlayForWork";
import { ApiDropdown } from "@/app/apis/[apiId]/api-dropdown";
import { VersionFilter } from "@/app/apis/[apiId]/apis-filter";
import CustomModal from "../CustomModal";
import CreateNewApiVersion from "../create-api-new-version";
import DeploymentCreationPage from "../deployment-creation";
import NewRouteForm from "../new-route";
import DeleteApiVersion from "../api-version-deletion";
import { downloadApi } from "@/lib/hooks/use-api-definitons";

const actionsMap = {
  new_version: "Create New Version",
  new_route: "Create New Route",
  delete_api_version: "Delete Api Version",
  deployment: "Create New Deployment",
} as Record<string, string>;
type secondaryHeaderProps = {
  onClick: () => void;
  variant: string;
  id?: string;
  apiTab?: string;
};

export default function SecondaryHeader({
  onClick,
  variant,
  id,
  apiTab,
}: secondaryHeaderProps) {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const pathname = usePathname();
  const { compId } = useParams<{ compId: string }>();
  const { id: workerName } = useParams<{ id: string }>();

  const { apiId } = useParams<{ apiId: string }>();
  const params = useSearchParams();
  const version = params.get("version");

  const [open, setOpen] = useState<string | null>(null);

  // const tab = useMemo(() => {
  //   const parts = pathname?.split("/") || [];
  //   return parts[parts.length - 1] || "overview";
  // }, [pathname]);

  const router = useRouter();
  let navigationLinks;
  if (variant === "apis") {
    navigationLinks = [
      {
        name: "Overview",
        href: `/apis/${apiId}/overview${version ? `?version=${version}` : ""}`,
        icon: <Home fontSize="small" />,
      },
      {
        name: "Settings",
        href: `/apis/${apiId}/settings${version ? `?version=${version}` : ""}`,
        icon: <Settings fontSize="small" />,
      },
      {
        name: "Deployments",
        href: `/apis/${apiId}/deployments${
          version ? `?version=${version}` : ""
        }`,
        icon: <RocketLaunch fontSize="small" />,
      },
      {
        name: "Playground",
        href: `/apis/${apiId}/playground${
          version ? `?version=${version}` : ""
        }`,
        icon: <PlayForWorkIcon fontSize="small" />,
      },
    ];
  } else {
    navigationLinks = [
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
  }

  const dropdowns = [
    {
      heading: "Create",
      list: [
        {
          label: "New Route",
          onClick: () => setOpen("new_route"),
        },
        { label: "New Version", onClick: () => setOpen("new_version") },
      ],
    },
    {
      heading: "Actions",
      list: [
        { label: "Deploy API", onClick: () => setOpen("deployment") },
        {
          label: "Download API",
          onClick: () => {
            downloadApi(apiId, version ?? "");
          },
        },
      ],
    },
    {
      heading: "Delete",
      list: [
        {
          label: "Delete All Routes",
          onClick: () => router.push(`/apis/${apiId}/delete-routes`),
        },
        {
          label: "Delete Version",
          onClick: () => setOpen("delete_api_version"),
        },
        {
          label: "Delete All Versions",
          onClick: () => router.push(`/apis/${apiId}/delete-all-versions`),
        },
      ],
    },
  ];

  useEffect(() => {
    const handleResize = () => {
      const screenWidth = window.innerWidth;
      if (screenWidth >= 895 && drawerOpen) {
        setDrawerOpen((prev) => !prev);
      }
    };

    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, [drawerOpen]);

  const workloads = [
    { route: `/components/${compId}/settings?activeTab=1`, value: "info" },
    { route: `/components/${compId}/settings?activeTab=2`, value: "update" },
  ];

  const toggleDrawer = (open: boolean) => () => {
    setDrawerOpen(open);
  };

  const handleClose = () => setOpen(null);

  const ApiName = decodeURIComponent(apiId);

  return (
    <Box className="dark:bg-[#0a0a0a] border-b p-2 pr-20 ">
      <Box className="flex items-center justify-between w-full">
        <Box
          sx={{
            display:
              apiTab === "playground" ? "block" : { xs: "block", md: "none" },
          }}
        >
          <Button
            startIcon={<PanelRightClose />}
            onClick={toggleDrawer(true)}
            className="dark:text-white text-9xl ml-2"
          ></Button>
        </Box>

        {variant === "apis" && (
          <Box className="flex gap-3 align-middle">
            <Typography variant="body2" sx={{ fontWeight: "bold" }}>
              {ApiName}
            </Typography>
            {apiTab != "playground" && <VersionFilter />}
          </Box>
        )}

        {variant === "apis" && apiTab != "playground" && (
          <Box className="border border-border rounded-md cursor-pointer dark:hover:bg-[#333] hover:bg-slate-100 py-1 px-2">
            <ApiDropdown dropdowns={dropdowns} />
          </Box>
        )}

        {pathname === `/components/${compId}/overview` && (
          <Box sx={{ marginLeft: "auto", display: "flex", gap: 2 }}>
            <Button2
              variant="primary"
              startIcon={<AddIcon />}
              size="md"
              onClick={onClick}
            >
              New
            </Button2>

            <Box className="border border-border rounded-md cursor-pointer dark:hover:bg-[#333] hover:bg-slate-100 py-1 px-2">
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
          {navigationLinks?.map((link) => {
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
      <CustomModal
        open={!!actionsMap[open!]}
        onClose={() => setOpen(null)}
        heading={`${actionsMap[open!]}`}
      >
        {open == "new_version" && (
          <CreateNewApiVersion
            apiId={apiId}
            version={version}
            onSuccess={handleClose}
          />
        )}
        {open == "deployment" && (
          <DeploymentCreationPage apiId={apiId} onSuccess={handleClose} />
        )}
        {open == "new_route" && (
          <div className="p-5">
            <NewRouteForm
              apiId={apiId}
              version={version}
              onSuccess={handleClose}
            />
          </div>
        )}
        {open == "delete_api_version" && (
          <DeleteApiVersion
            apiId={apiId}
            version={version}
            onSuccess={handleClose}
          />
        )}
      </CustomModal>
    </Box>
  );
}
