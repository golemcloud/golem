"use client";
import Sidebar from "@/components/ui/Sidebar";
import {
  useParams,
  usePathname,
  useRouter,
  useSearchParams,
} from "next/navigation";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import PlayForWorkIcon from "@mui/icons-material/PlayForWork";
import { useMemo, useState } from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { Loader } from "lucide-react";
import {
  Stack,
  Typography,
  Select,
  MenuItem,
  Button,
} from "@mui/material";
import CreateNewApiVersion from "@/components/create-api-new-version";
import CustomModal from "@/components/CustomModal";

export default function APISLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const { apiId } = useParams<{ apiId: string }>();
  const params = useSearchParams();
  const version = params.get("version");
  const pathname = usePathname();
  const [open, setOpen] = useState(false);
  const router = useRouter();
  const { apiDefinitions, getApiDefintion, isLoading } =
    useApiDefinitions(apiId);
  const { data: apiDefinition } = getApiDefintion(apiId, version);
  const versions = useMemo(() => {
    return apiDefinitions.map((api) => {
      return api.version;
    });
  }, [apiDefinitions]);

  const navigationLinks = useMemo(() => {
    return [
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
  }, [apiId, apiDefinition?.version]);

  const tab = useMemo(() => {
    const parts = pathname?.split("/") || [];
    return parts[parts.length - 1] || "overview"; // Default to "overview"
  }, [pathname]);

  if (isLoading) {
    return <Loader />;
  }

  return (
    <div style={{ display: "flex"}}>
      <Sidebar id={apiId!} navigationLinks={navigationLinks} variant="apis" />
      <div
        className="flex-1 p-[20px]"
        // style={{
        //   overflowY: "auto",
        //   height: "100vh",
        // }}
      >
        {tab !== "playground" && (
          <>
            <Stack direction="row" alignItems={"center"} marginBottom={2}>
              <Typography className="">{apiDefinition?.id}</Typography>
              <Select
                name="version"
                variant="outlined"
                className="w-32 ml-2"
                value={apiDefinition?.version}
                onChange={(e) => {
                  if(apiDefinition && e.target.value!== apiDefinition?.version){
                    router.push(`/apis/${apiId}/${tab}?version=${e.target.value}`);

                  }
                }}
              >
                {versions?.map((version: string) => (
                  <MenuItem key={version} value={version}>
                    {version}
                  </MenuItem>
                ))}
              </Select>
              {/* TODO: need to add menu for new api creation, deletion and other operations */}
              <Button
                type="button"
                className="ml-auto"
                onClick={(e) => {
                  e.preventDefault();
                  setOpen(true);
                }}
              >
                New Version
              </Button>
            </Stack>
          </>
        )}
        <CustomModal open={open} onClose={() => setOpen(false)} heading={"Create New version"}>
              {apiDefinition && <CreateNewApiVersion apiId={apiId} version={apiDefinition.version} onSuccess={()=>setOpen(false)}/>}
        </CustomModal>
        {children}
      </div>
    </div>
  );
}
