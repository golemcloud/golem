"use client";

import { ChevronRight, type LucideIcon } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { useNavigate } from "react-router-dom";

export interface SidebarMenuProps {
  title: string;
  url: string;
  icon?: LucideIcon;
  isActive?: boolean;
  isHidden?: boolean;
  items?: SidebarMenuProps[];
}

export function NavMain({
  items,
  setActiveItem,
  activeItem,
}: {
  items: SidebarMenuProps[];
  activeItem?: string;
  setActiveItem?: (item: string) => void;
}) {
  const navigate = useNavigate();

  const onSelect = (item: SidebarMenuProps) => {
    if (setActiveItem) setActiveItem(item.title);
    navigate(item.url);
  };

  // Define your active/inactive classes.
  // Feel free to adjust these classes to match your dark theme.
  const activeClasses =
    "bg-gray-300 dark:bg-neutral-800 text-gray-900 dark:text-gray-100";
  const inactiveClasses =
    "hover:bg-gray-200 dark:hover:bg-neutral-700 text-gray-700 dark:text-gray-300";

  return (
    <SidebarGroup>
      <SidebarMenu>
        {items.map(
          item =>
            !item.isHidden &&
            (item.items ? (
              // Render collapsible menu for items with sub-items
              <Collapsible
                key={item.title}
                asChild
                className="group/collapsible"
              >
                <SidebarMenuItem>
                  <CollapsibleTrigger asChild>
                    <SidebarMenuButton
                      tooltip={item.title}
                      className={`flex items-center gap-2 ${
                        activeItem === item.title
                          ? activeClasses
                          : inactiveClasses
                      }`}
                    >
                      {item.icon && <item.icon />}
                      <span>{item.title}</span>
                      <ChevronRight className="ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90" />
                    </SidebarMenuButton>
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <SidebarMenuSub>
                      {item.items.map(subItem => (
                        <SidebarMenuSubItem
                          key={subItem.title}
                          className={`${
                            subItem.isActive ? activeClasses : inactiveClasses
                          }`}
                        >
                          <SidebarMenuSubButton
                            onClick={() => onSelect(subItem)}
                          >
                            <span>{subItem.title}</span>
                          </SidebarMenuSubButton>
                        </SidebarMenuSubItem>
                      ))}
                    </SidebarMenuSub>
                  </CollapsibleContent>
                </SidebarMenuItem>
              </Collapsible>
            ) : (
              // Render a simple link when no sub-items exist
              <SidebarMenuItem key={item.title}>
                <SidebarMenuButton
                  onClick={() => onSelect(item)}
                  className={`flex items-center gap-2 ${
                    activeItem === item.title ? activeClasses : inactiveClasses
                  }`}
                >
                  {item.icon && <item.icon />}
                  <span>{item.title}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            )),
        )}
      </SidebarMenu>
    </SidebarGroup>
  );
}
