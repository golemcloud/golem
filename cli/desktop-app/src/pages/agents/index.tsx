import { useEffect, useState } from "react";
import { LayoutGrid, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card.tsx";
import { useNavigate, useParams } from "react-router-dom";
import { Agent } from "@/types/agent.ts";
import { API } from "@/service";

const WORKER_COLOR_MAPPER = {
  Idle: "text-emerald-400 dark:text-emerald-200",
  Running: "text-blue-400 dark:text-blue-200",
  Suspended: "text-amber-400 dark:text-amber-200",
  Failed: "text-rose-400 dark:text-rose-200",
};

export default function AgentList() {
  const [agentList, setAgentList] = useState<Agent[]>([]);
  const [filteredAgents, setFilteredAgents] = useState<Agent[]>([]);
  const [searchQuery, setSearchQuery] = useState("");

  const navigate = useNavigate();
  const { appId, componentId } = useParams();

  useEffect(() => {
    API.agentService.findAgent(appId!, componentId!).then(res => {
      const sortedData = res.workers.sort(
        (a: Agent, b: Agent) =>
          new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime(),
      );
      setAgentList(sortedData);
      setFilteredAgents(sortedData);
    });
  }, [componentId]);

  useEffect(() => {
    const lowerCaseQuery = searchQuery.toLowerCase();
    const filtered = agentList.filter(
      (agent: Agent) =>
        agent.workerName?.toLowerCase().includes(lowerCaseQuery) ||
        agent.status?.toLowerCase().includes(lowerCaseQuery),
    );
    setFilteredAgents(filtered);
  }, [searchQuery, agentList]);

  return (
    <div className="flex">
      <div className="flex-1 p-8">
        <div className="p-6 bg-background text-foreground mx-auto max-w-7xl rounded-lg border border-border shadow-sm">
          <div className="flex gap-4 mb-4 items-center">
            <div className="relative flex-1">
              <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
              <Input
                className="w-full pl-10 bg-muted rounded-md"
                placeholder="Search agents..."
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
              />
            </div>
            <Button
              variant="default"
              onClick={() =>
                navigate(
                  `/app/${appId}/components/${componentId}/agents/create`,
                )
              }
            >
              <Plus className="h-4 w-4" />
              New Agent
            </Button>
          </div>

          {filteredAgents.length === 0 ? (
            <div className="border-2 border-dashed border-gray-300 dark:border-gray-700 rounded-lg p-12 flex flex-col items-center justify-center">
              <div className="h-16 w-16 bg-gray-100 dark:bg-gray-800 rounded-lg flex items-center justify-center mb-4">
                <LayoutGrid className="h-8 w-8 text-gray-400" />
              </div>
              <h2 className="text-xl font-semibold text-foreground">
                No Agents Found
              </h2>
              <p className="text-muted-foreground">
                Create a new agent to get started.
              </p>
            </div>
          ) : (
            <div className="overflow-auto max-h-[70vh] space-y-4">
              {filteredAgents.map((agent: Agent, index) => (
                <Card
                  key={index}
                  className="rounded-lg border border-border bg-muted hover:bg-muted/80 hover:shadow-lg transition cursor-pointer"
                  onClick={() =>
                    navigate(
                      `/app/${appId}/components/${componentId}/agents/${agent.workerName}`,
                    )
                  }
                >
                  <CardHeader>
                    <CardTitle className="text-foreground font-mono">
                      {agent.workerName}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className="py-2">
                    <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Status
                        </div>
                        <div
                          className={`text-lg font-semibold ${WORKER_COLOR_MAPPER[agent.status as keyof typeof WORKER_COLOR_MAPPER]}`}
                        >
                          {agent.status}
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Memory
                        </div>
                        <div className="text-lg font-semibold">
                          {agent.totalLinearMemorySize / 1024} KB
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Pending Invocations
                        </div>
                        <div className="text-lg font-semibold">
                          {agent.pendingInvocationCount}
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Version
                        </div>
                        <div className="text-lg font-semibold">
                          v{agent.componentRevision}
                        </div>
                      </div>
                    </div>
                    <div className="py-2 flex gap-2 text-sm">
                      {/* <Badge variant="outline" className="rounded-sm">
                        Args: {agent.args.length}
                      </Badge> */}
                      <Badge variant="outline" className="rounded-sm">
                        Env: {Object.keys(agent.env).length}
                      </Badge>
                      <span className="text-muted-foreground ml-auto">
                        {new Date(agent.createdAt).toLocaleString()}
                      </span>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
