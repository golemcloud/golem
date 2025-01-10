"use client";
import Sidebar from "@/components/ui/Sidebar";
import {
  usePathname,
  useSearchParams,
} from "next/navigation";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import PlayForWorkIcon from "@mui/icons-material/PlayForWork";
import { useMemo } from "react";
import SecondaryHeader from "@/components/ui/secondary-header";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

export default function APISLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const { apiId } = useCustomParam();
  const params = useSearchParams();
  const version = params.get("version");
  const pathname = usePathname();
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
  }, [apiId, version]);

  const tab = useMemo(() => {
    const parts = pathname?.split("/") || [];
    return parts[parts.length - 1] || "overview";
  }, [pathname]);

  return (
    <div style={{ display: "flex" }}>
      <Sidebar id={apiId!} navigationLinks={navigationLinks} variant="apis" apiTab={tab} />
      <div
        className="flex-1 "
      >
        <SecondaryHeader
          variant="apis"
          apiTab={tab}
        />
        <div className="mx-auto max-w-7xl px-2 md:px-4 lg:px-8">
          <div className="mx-auto max-w-2xl lg:max-w-none py-5 md:p-5">
            {children}
          </div>
        </div>
      </div>
    </div>
  ); 
}
