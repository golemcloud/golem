import {ArrowLeft, type LucideIcon,} from "lucide-react"
import {
    Sidebar,
    SidebarContent,
    SidebarHeader,
    SidebarMenu as SidebarMenuM,
    SidebarMenuButton,
    SidebarMenuItem,
    SidebarRail,
} from "@/components/ui/sidebar"
import {NavMain, SidebarMenuProps} from "@/components/nav-main.tsx";
import {useNavigate} from "react-router-dom";

export interface SidebarProps {
    headers?: SidebarHeaderProps[]
    menus: SidebarMenuProps[]
    footer?: { name: string; url: string }[]
    side?: "left" | "right"
    variant?: "sidebar" | "floating" | "inset"
    collapsible?: "offcanvas" | "icon" | "none"
    className?: string
    setActiveItem?: (item: string) => void
}

export interface SidebarHeaderProps {
    name?: string
    logo?: LucideIcon
    plan?: string
}

export function SidebarMenu({...props}: SidebarProps) {

    const navigate = useNavigate();
    return (
        <Sidebar collapsible="icon" {...props}>
            <SidebarHeader>
                <SidebarMenuM>
                    <SidebarMenuItem className="flex items-center space-x-2">
                        <SidebarMenuButton onClick={() => navigate(-1)}>
                            <ArrowLeft/>
                            <span>Component</span>
                        </SidebarMenuButton>
                    </SidebarMenuItem>
                </SidebarMenuM>
                {/*<TeamSwitcher teams={data.teams}/>*/}
            </SidebarHeader>
            <SidebarContent>
                <NavMain items={props.menus} setActiveItem={props.setActiveItem}/>
                {/*<NavProjects projects={props.menus}/>*/}
            </SidebarContent>
            <SidebarRail/>
        </Sidebar>
    )
}
