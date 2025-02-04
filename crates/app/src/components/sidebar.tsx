import { type LucideIcon } from "lucide-react";
import { Sidebar, SidebarContent, SidebarRail } from "@/components/ui/sidebar";
import { NavMain, SidebarMenuProps } from "@/components/nav-main.tsx";

export interface SidebarProps {
  headers?: SidebarHeaderProps[];
  menus: SidebarMenuProps[];
  footer?: { name: string; url: string }[];
  side?: "left" | "right";
  variant?: "sidebar" | "floating" | "inset";
  collapsible?: "offcanvas" | "icon" | "none";
  className?: string;
  setActiveItem?: (item: string) => void;
  activeItem?: string;
  title?: string;
}

export interface SidebarHeaderProps {
  name?: string;
  logo?: LucideIcon;
  plan?: string;
}

export function SidebarMenu({ ...props }: SidebarProps) {
  return (
    <Sidebar collapsible="icon" {...props}>
      {/*<SidebarHeader>*/}
      {/*<SidebarMenuM>*/}
      {/*    <SidebarMenuItem className="flex items-center space-x-2">*/}
      {/*        <SidebarMenuButton onClick={() => navigate(-1)}>*/}
      {/*            <ArrowLeft/>*/}
      {/*            <span>{props.title}</span>*/}
      {/*        </SidebarMenuButton>*/}
      {/*    </SidebarMenuItem>*/}
      {/*</SidebarMenuM>*/}
      {/*<TeamSwitcher teams={data.teams}/>*/}
      {/*</SidebarHeader>*/}
      <SidebarContent>
        <NavMain
          items={props.menus}
          setActiveItem={props.setActiveItem}
          activeItem={props.activeItem}
        />
        {/*<NavProjects projects={props.menus}/>*/}
      </SidebarContent>
      <SidebarRail />
    </Sidebar>
  );
}
