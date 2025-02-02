"use client";

import {ChevronRight, type LucideIcon} from "lucide-react";

import {Collapsible, CollapsibleContent, CollapsibleTrigger,} from "@/components/ui/collapsible";
import {
    SidebarGroup,
    SidebarGroupLabel,
    SidebarMenu,
    SidebarMenuButton,
    SidebarMenuItem,
    SidebarMenuSub,
    SidebarMenuSubButton,
    SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import {useNavigate} from "react-router-dom";

export interface SidebarMenuProps {
    title: string
    url: string
    icon?: LucideIcon
    isActive?: boolean
    isHidden?: boolean
    items?: SidebarMenuProps[]
}

export function NavMain({
                            items, setActiveItem
                        }: {
    items: SidebarMenuProps[];
    setActiveItem?: (item: string) => void
}) {
    const navigate = useNavigate();

    const onSelect = (item: SidebarMenuProps) => {
        if (setActiveItem) setActiveItem(item.title);
        navigate(item.url);
    };
    return (
        <SidebarGroup>
            <SidebarGroupLabel>Menu</SidebarGroupLabel>
            <SidebarMenu>
                {items.map((item) =>
                        !item.isHidden && ( // If there are sub-items, render a collapsible menu
                            item.items ? ( // If there are sub-items, render a collapsible menu
                                <Collapsible
                                    key={item.title}
                                    asChild
                                    defaultOpen={item.isActive}
                                    className="group/collapsible"
                                >
                                    <SidebarMenuItem>
                                        <CollapsibleTrigger asChild>
                                            <SidebarMenuButton tooltip={item.title}>
                                                {item.icon && <item.icon/>} {/* Ensure icon exists before rendering */}
                                                <span>{item.title}</span>
                                                <ChevronRight
                                                    className="ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90"/>
                                            </SidebarMenuButton>
                                        </CollapsibleTrigger>
                                        <CollapsibleContent>
                                            <SidebarMenuSub>
                                                {item.items.map((subItem) => (
                                                    <SidebarMenuSubItem key={subItem.title}>
                                                        <SidebarMenuSubButton onClick={() => onSelect(subItem)}>
                                                            <span>{subItem.title}</span>
                                                        </SidebarMenuSubButton>
                                                    </SidebarMenuSubItem>
                                                ))}
                                            </SidebarMenuSub>
                                        </CollapsibleContent>
                                    </SidebarMenuItem>
                                </Collapsible>
                            ) : ( // If no sub-items, render a simple link
                                <SidebarMenuItem key={item.title}>
                                    <SidebarMenuButton onClick={() => onSelect(item)}>
                                        {item.icon && <item.icon/>}
                                        <span>{item.title}</span>
                                    </SidebarMenuButton>
                                </SidebarMenuItem>
                            )
                        )
                )}
            </SidebarMenu>
        </SidebarGroup>
    );
}