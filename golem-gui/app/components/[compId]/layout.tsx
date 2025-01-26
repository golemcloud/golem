
"use client"

import Sidebar from "@/components/ui/Sidebar";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import CodeIcon from '@mui/icons-material/Code';
import ArticleIcon from '@mui/icons-material/Article';
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { useSearchParams } from "next/navigation";
import { useMemo } from "react";
import useComponents from "@/lib/hooks/use-component";

export default function ComponentsLayout({
  children
}: {
  children: React.ReactNode;
}) {

   const { compId } = useCustomParam();
   const params = useSearchParams();
   const version = params?.get("version");
   const component = useComponents(compId!, version);
   const type = component?.components[0]?.componentType;

   const navigationLinks = useMemo(()=>[
    { name: "Overview", href: `/components/${compId}/overview${version? `?version=${version}`: ''}`, icon: <Home fontSize="small" /> },
    { name: "Workers", href: `/components/${compId}/workers`, icon: <CodeIcon fontSize="small" /> },
    { name: "Exports", href: `/components/${compId}/exports`, icon: <RocketLaunch fontSize="small" /> },
    { name: "Files", href: `/components/${compId}/files`, icon: <ArticleIcon fontSize="small" /> },
    { name: "Settings", href: `/components/${compId}/settings`, icon: <Settings fontSize="small" /> },
  ], [compId, version]);

  return (
    <div style={{ display: "flex"}}>
      <Sidebar id={compId!} navigationLinks={navigationLinks} variant="components" type={type}/>
      <div className="flex-1">
        {children}
      </div>
    </div>
  );
}
