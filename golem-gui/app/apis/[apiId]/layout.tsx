"use client"

import Sidebar from "@/components/ui/Sidebar";
import { useParams } from "next/navigation";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";


export default function APISLayout({
  children,
}: {
  children: React.ReactNode;
}) {

  const { apiId } = useParams<{apiId:string}>();

   const navigationLinks = [
    { name: "Overview", href: `/apis/${apiId}/overview`, icon: <Home fontSize="small" /> },
    { name: "Settings", href: `/apis/${apiId}/settings`, icon: <Settings fontSize="small" /> },
    { name: "Deployments", href: `/apis/${apiId}/deployments`, icon: <RocketLaunch fontSize="small" /> },
  ];

  return (
    
    <div style={{ display: "flex", height: "100vh", overflow: "hidden" }}>
      <Sidebar id={apiId!} navigationLinks={navigationLinks} variant="apis" />
      <div
        className="flex-1 p-[20px]"
        style={{
          overflowY: "auto",
          height: "100vh",
        }}
      >
        {children}
      </div>
    </div>
  );
}
