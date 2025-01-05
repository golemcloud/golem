"use client";
import Sidebar from "@/components/ui/Sidebar";
import {
  useParams,
  usePathname,
  useSearchParams,
} from "next/navigation";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import PlayForWorkIcon from "@mui/icons-material/PlayForWork";
import { useMemo, useState } from "react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { Loader } from "lucide-react";
import CreateNewApiVersion from "@/components/create-api-new-version";
import CustomModal from "@/components/CustomModal";
import SecondaryHeader from "@/components/ui/secondary-header";

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
  const { getApiDefintion, isLoading } =
    useApiDefinitions(apiId);
  const { data: apiDefinition } = getApiDefintion(apiId, version);
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
    return parts[parts.length - 1] || "overview";
  }, [pathname]);

  if (isLoading) {
    return <Loader />;
  }

  return (
    <div style={{ display: "flex" }}>
      <Sidebar id={apiId!} navigationLinks={navigationLinks} variant="apis" apiTab={tab} />
      <div
        className="flex-1 "
        // style={{
        //   overflowY: "auto",
        //   height: "100vh",
        // }}
      >
        <SecondaryHeader
          variant="apis"
          onClick={() => {
            setOpen(true);
          }}
          apiTab={tab}
        />

        <div className="mx-auto max-w-7xl px-6 lg:px-8">
          <div className="mx-auto max-w-2xl lg:max-w-none p-5">
            <CustomModal
              open={open}
              onClose={() => setOpen(false)}
              heading={"Create New version"}
            >
              {apiDefinition && (
                <CreateNewApiVersion
                  apiId={apiId}
                  version={apiDefinition.version}
                  onSuccess={() => setOpen(false)}
                />
              )}
            </CustomModal>
            {children}
          </div>
        </div>
      </div>
    </div>
  ); 
}
