import {Plus,} from "lucide-react"
import {
    SidebarGroup,
    SidebarGroupLabel,
    SidebarMenu,
    SidebarMenuButton,
    SidebarMenuItem,
} from "@/components/ui/sidebar"
import {Badge} from "@/components/ui/badge.tsx";
import {Button} from "@/components/ui/button.tsx";
import {useNavigate} from "react-router-dom";

export const HTTP_METHOD_COLOR = {
    Get: "bg-emerald-900 text-emerald-200 hover:bg-emerald-900",
    Post: "bg-gray-900 text-gray-200 hover:bg-gray-900",
    Put: "bg-yellow-900 text-yellow-200 hover:bg-yellow-900",
    Patch: "bg-blue-900 text-blue-200 hover:bg-blue-900",
    Delete: "bg-red-900 text-red-200 hover:bg-red-900",
    Head: "bg-purple-900 text-purple-200 hover:bg-purple-900",
    Options: "bg-indigo-900 text-indigo-200 hover:bg-indigo-900",
    Trace: "bg-pink-900 text-pink-200 hover:bg-pink-900",
    Connect: "bg-sky-900 text-sky-200 hover:bg-sky-900",
};

export function NavRoutes({
                              routes,
                              setActiveItem,
                              newRouteEndpoint,
                          }: {
    routes: {
        method: string
        name: string
        url: string
    }[]
    setActiveItem: (item: string) => void
    newRouteEndpoint: string
}) {
    const navigate = useNavigate();

    return (
        <SidebarGroup className="group-data-[collapsible=icon]:hidden">
            <SidebarGroupLabel>Routes</SidebarGroupLabel>
            <SidebarMenu>
                {routes.map((item) => (
                    <SidebarMenuItem key={`${item.name}-${item.method}`}>
                        <SidebarMenuButton onClick={() => setActiveItem(item.name)} asChild>
                            <a href={item.url}>
                                <Badge
                                    variant="secondary"
                                    className={
                                        HTTP_METHOD_COLOR[
                                            item.method as keyof typeof HTTP_METHOD_COLOR
                                            ]
                                    }
                                >
                                    {item.method}
                                </Badge>
                                <span>{item.name}</span>
                            </a>
                        </SidebarMenuButton>

                    </SidebarMenuItem>
                ))}
                <SidebarMenuItem>
                    <Button
                        variant="outline"
                        onClick={() =>
                            navigate(newRouteEndpoint)
                        }
                        className="flex items-center justify-center text-sm px-3 py-2 w-full rounded-lg border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-100 hover:border-gray-400 dark:hover:border-gray-500"
                    >
                        <Plus className="h-5 w-5"/>
                        <span>Add</span>
                    </Button>
                    {/*<SidebarMenuButton className="text-sidebar-foreground/70">*/}

                    {/*</SidebarMenuButton>*/}
                </SidebarMenuItem>
            </SidebarMenu>
        </SidebarGroup>
    )
}
