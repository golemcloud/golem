import Sidebar from "@ui/Sidebar";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import CodeIcon from '@mui/icons-material/Code';
import ArticleIcon from '@mui/icons-material/Article';
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { Outlet, useSearchParams } from "react-router-dom";
import { useMemo, useEffect, useState } from "react";
import useComponents from "@lib/hooks/use-component";

export default function ComponentsLayout() {
   const { compId } = useCustomParam();
   const [params] = useSearchParams();
   const version = params?.get("version");
   const component = useComponents(compId!, version);
   const type = component?.components[0]?.componentType;
   const [typecomp, setTypecomp] = useState("");
   
   useEffect(() => {
     if (type === "Ephemeral") {
       setTypecomp("ephemeraloverview");
     } else {
       setTypecomp("durableoverview");
     }
   }, [type]);

   const navigationLinks = useMemo(() => [
    { 
      name: "Overview", 
      href: `/components/${compId}/${typecomp}${version ? `?version=${version}` : ''}`, 
      icon: <Home fontSize="small" /> 
    },
    { 
      name: "Workers", 
      href: `/components/${compId}/workers`, 
      icon: <CodeIcon fontSize="small" /> 
    },
    { 
      name: "Exports", 
      href: `/components/${compId}/exports`, 
      icon: <RocketLaunch fontSize="small" /> 
    },
    { 
      name: "Files", 
      href: `/components/${compId}/files`, 
      icon: <ArticleIcon fontSize="small" /> 
    },
    { 
      name: "Settings", 
      href: `/components/${compId}/settings`, 
      icon: <Settings fontSize="small" /> 
    },
   ], [compId, version, typecomp]); // Added typecomp as dependency

  return (
    <div style={{ display: "flex" }}>
      <Sidebar 
        id={compId!} 
        navigationLinks={navigationLinks} 
        variant="components" 
        type={type}
      />
      <div className="flex-1">
        <Outlet />
      </div>
    </div>
  );
}