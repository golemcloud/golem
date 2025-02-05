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
  children?: React.ReactNode;
}

export interface SidebarHeaderProps {
  name?: string;
  logo?: LucideIcon;
  plan?: string;
}

export function SidebarMenu({ ...props }: SidebarProps) {
  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarContent>
        <NavMain
          items={props.menus}
          setActiveItem={props.setActiveItem}
          activeItem={props.activeItem}
        />
        {props.children}
      </SidebarContent>
      <SidebarRail />
    </Sidebar>
  );
}
