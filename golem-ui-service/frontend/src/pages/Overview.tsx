import {
  Activity,
  Box,
  ChevronRight,
  Crown,
  Globe,
  Package,
  Puzzle,
  Server,
  Terminal,
} from "lucide-react";

import { Link } from "react-router-dom";
import { useApiDefinitions } from "../api/api-definitions";
import { useComponents } from "../api/components";
import { usePlugins } from "../api/plugins";

const SectionCard = ({
  title,
  viewMoreLink,
  icon: Icon,
  children,
}: {
  title: string;
  viewMoreLink: string;
  icon: React.ComponentType<{ size: number }>;
  children: React.ReactNode;
}) => (
  <div className="bg-card rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow duration-200">
    <div className="flex justify-between items-center mb-6">
      <div className="flex items-center gap-3">
        <div className="p-2 rounded-md bg-card/50 text-primary">
          <Icon size={20} />
        </div>
        <h2 className="text-xl font-semibold text-foreground/90">{title}</h2>
      </div>
      <Link
        to={viewMoreLink}
        className="flex items-center gap-1 text-sm text-primary hover:text-primary-accent transition-colors
                 px-3 py-1 rounded-md hover:bg-card/50"
      >
        View all
        <ChevronRight size={16} />
      </Link>
    </div>
    {children}
  </div>
);

const ListItem = ({
  title,
  subtitle,
  status,
  icon: Icon,
  url,
}: {
  title: string;
  subtitle?: string;
  status?: "active" | "inactive" | "error";
  icon?: React.ComponentType<{ size: number }>;
  url?: string;
}) => {
  const statusColors = {
    active: "bg-green-500",
    inactive: "bg-gray-500",
    error: "bg-red-500",
  };

  return (
    <div
      className="border-t border-gray-700 py-4 flex items-center justify-between group hover:bg-card/30 px-3 
                    rounded-md transition-colors -mx-3"
    >
      <div className="flex items-center gap-3">
        {Icon && (
          <div className="text-muted-foreground group-hover:text-primary transition-colors">
            <Icon size={16} />
          </div>
        )}
        <div>
          <Link to={url!}>
            <h3 className="text-sm font-medium text-primary/80 group-hover:text-secondary transition-colors">
              {title}
            </h3>
          </Link>
          {subtitle && (
            <p className="text-xs text-muted-foreground">{subtitle}</p>
          )}
        </div>
      </div>
      {status && (
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground group-hover:opacity-100 opacity-0 transition-opacity">
            {status.charAt(0).toUpperCase() + status.slice(1)}
          </span>
          <div className={`w-2 h-2 rounded-full ${statusColors[status]}`} />
        </div>
      )}
    </div>
  );
};

const LoadingState = () => (
  <div className="flex items-center justify-center py-8">
    <Activity className="animate-spin text-muted-foreground" size={20} />
  </div>
);

const EmptyState = ({ message }: { message: string }) => (
  <div className="text-center py-8">
    <Box className="mx-auto text-gray-600 mb-2" size={24} />
    <p className="text-muted-foreground text-sm">{message}</p>
  </div>
);

export const Overview = () => {
  const { data: apis, isLoading: apisLoading } = useApiDefinitions();
  const { data: components, isLoading: componentsLoading } = useComponents();
  const { data: plugins, isLoading: pluginsLoading } = usePlugins();

  document.title = `Overview - Golem UI`;

  return (
    <div className="space-y-8">
      <div className="bg-card/50 p-6 rounded-lg">
        <h1 className="text-2xl font-bold flex items-center gap-3">
          <Terminal size={24} className="text-primary" />
          Overview
        </h1>
        <p className="text-muted-foreground mt-1">
          Monitor and manage your system components
        </p>
      </div>

      {/* Grid section */}
      <div className="grid md:grid-cols-8 gap-6">
        {/* APIs - 3/8 width */}
        <div className="col-span-3">
          <SectionCard title="APIs" viewMoreLink="/apis" icon={Terminal}>
            {apisLoading ? (
              <LoadingState />
            ) : (
              <div className="space-y-1">
                {apis
                  ?.slice(0, 5)
                  .map((api) => (
                    <ListItem
                      key={api.id}
                      title={api.id}
                      subtitle={`Version ${api.version}`}
                      status={api.draft ? "inactive" : "active"}
                      icon={Globe}
                      url={"/apis/definitions/" + api.id + "/" + api.version}
                    />
                  ))}
                {!apis?.length && <EmptyState message="No APIs defined" />}
              </div>
            )}
          </SectionCard>
        </div>

        {/* Components - 5/8 width */}
        <div className="col-span-5">
          <SectionCard
            title="Components"
            viewMoreLink="/components"
            icon={Package}
          >
            {componentsLoading ? (
              <LoadingState />
            ) : (
              <div className="space-y-1">
                {components
                  ?.slice(0, 5)
                  .map((component) => (
                    <ListItem
                      key={component.versionedComponentId.componentId + component.versionedComponentId.version}
                      title={component.componentName}
                      subtitle={`Version ${component.versionedComponentId.version}`}
                      status="active"
                      icon={Server}
                      url={`/components/${component.versionedComponentId.componentId}/${component.versionedComponentId.version}`}
                    />
                  ))}
                {!components?.length && (
                  <EmptyState message="No components available" />
                )}
              </div>
            )}
          </SectionCard>
        </div>
      </div>

      {/* Plugins section - Full width */}
      <SectionCard title="Plugins" viewMoreLink="/plugins" icon={Puzzle}>
        {pluginsLoading ? (
          <LoadingState />
        ) : (
          <div className="space-y-1">
            {plugins
              ?.slice(0, 5)
              .map((plugin) => (
                <ListItem
                  key={`${plugin.name}-${plugin.version}`}
                  title={plugin.name}
                  subtitle={`Version ${plugin.version}`}
                  status={
                    plugin.scope.type === "Global" ? "active" : "inactive"
                  }
                  icon={plugin.scope.type === "Global" ? Crown : Box}
                  url={`/plugins/${plugin.name}/${plugin.version}`}
                />
              ))}
            {!plugins?.length && <EmptyState message="No plugins installed" />}
          </div>
        )}
      </SectionCard>
    </div>
  );
};
