import {ChevronDown, Plus, Search} from 'lucide-react'
import {Button} from '@/components/ui/button'
import {Input} from '@/components/ui/input'
import {Badge} from '@/components/ui/badge'
import {Card, CardContent, CardHeader, CardTitle} from "@/components/ui/card.tsx";
import {useNavigate} from "react-router-dom";


export function PluginList() {
    const navigate = useNavigate()


    return (
        <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
            <div className="flex gap-2 mb-4">
                <div className="relative flex-1">
                    <Search
                        className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4"/>
                    <Input
                        className="w-full pl-10"
                        placeholder="Plugin name..."
                    />
                </div>
                <Button variant="default" onClick={() => navigate("/plugins/create")}>
                    <Plus className="h-4 w-4"/>
                    Create Plugin
                </Button>
            </div>
            <Card className="rounded-lg mb-4">
                <CardHeader>
                    <div className="flex justify-between items-center">
                        <CardTitle>dummy</CardTitle>
                        <ChevronDown className="h-4 w-4"/>
                    </div>
                </CardHeader>
                <CardContent className={"py-2"}>
                    <div className="pt-0 grid grid-cols-1 md:grid-cols-4 gap-4">
                        <div>
                            <div className="text-sm text-muted-foreground">Status</div>
                            <div className="flex items-center gap-1">
                                Idle
                                <svg className="h-3 w-3" viewBox="0 0 24 24">
                                    <path
                                        fill="currentColor"
                                        d="M13 10V3L4 14h7v7l9-11h-7z"
                                    />
                                </svg>
                            </div>
                        </div>

                        <div>
                            <div className="text-sm text-muted-foreground">Memory</div>
                            <div className="flex items-center gap-1">
                                1 MB
                                <svg className="h-3 w-3" viewBox="0 0 24 24">
                                    <path
                                        fill="currentColor"
                                        d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.41 0-8-3.59-8-8s3.59-8 8-8 8 3.59 8 8-3.59 8-8 8z"
                                    />
                                </svg>
                            </div>
                        </div>

                        <div>
                            <div className="text-sm text-muted-foreground">Pending Invocations</div>
                            <div className="flex items-center gap-1">
                                0
                                <svg className="h-3 w-3" viewBox="0 0 24 24">
                                    <path
                                        fill="currentColor"
                                        d="M19 3h-4.18C14.4 1.84 13.3 1 12 1c-1.3 0-2.4.84-2.82 2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-7 0c.55 0 1 .45 1 1s-.45 1-1 1-1-.45-1-1 .45-1 1-1z"
                                    />
                                </svg>
                            </div>
                        </div>

                        <div>
                            <div className="text-sm text-muted-foreground">Resources</div>
                            <div className="flex items-center gap-1">
                                0
                                <svg className="h-3 w-3" viewBox="0 0 24 24">
                                    <path
                                        fill="currentColor"
                                        d="M19 5v14H5V5h14m0-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z"
                                    />
                                </svg>
                            </div>
                        </div>
                    </div>
                    <div className="py-1 flex gap-2">
                        <Badge variant="outline" className="rounded-sm">v0</Badge>
                        <Badge variant="outline" className="rounded-sm">Env 0</Badge>
                        <Badge variant="outline" className="rounded-sm">Args 0</Badge>
                        <span className="text-sm text-muted-foreground ml-auto">
              less than a minute ago
            </span>
                    </div>
                </CardContent>

            </Card>
        </div>
    )
}

