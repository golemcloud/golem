
"use client"

import Sidebar from "@/components/ui/Sidebar";
import { Home, Settings, RocketLaunch } from "@mui/icons-material";
import CodeIcon from '@mui/icons-material/Code';
import ArticleIcon from '@mui/icons-material/Article';
import { useCustomParam } from "@/lib/hooks/use-custom-param";



export default function ComponentsLayout({
  children,
}: {
  children: React.ReactNode;
}) {

   const { compId } = useCustomParam();

   const navigationLinks = [
    { name: "Overview", href: `/components/${compId}/overview`, icon: <Home fontSize="small" /> },
    { name: "Workers", href: `/components/${compId}/workers`, icon: <CodeIcon fontSize="small" /> },
    { name: "Exports", href: `/components/${compId}/exports`, icon: <RocketLaunch fontSize="small" /> },
    { name: "Files", href: `/components/${compId}/files`, icon: <ArticleIcon fontSize="small" /> },
    { name: "Settings", href: `/components/${compId}/settings`, icon: <Settings fontSize="small" /> },
  ];

  return (
    <div style={{ display: "flex"}}>
      <Sidebar id={compId!} navigationLinks={navigationLinks} variant="components" />
      <div className="flex-1">
        {children}
      </div>
    </div>
  );
}
