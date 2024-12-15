import { Link } from 'react-router-dom';
import { useApiDefinitions } from '../api/api-definitions';
import { useComponents } from '../api/components';
import { usePlugins } from '../api/plugins';

const SectionCard = ({ 
  title, 
  viewMoreLink, 
  children 
}: { 
  title: string; 
  viewMoreLink: string; 
  children: React.ReactNode;
}) => (
  <div className="bg-gray-800 rounded-lg shadow-lg p-6">
    <div className="flex justify-between items-center mb-4">
      <h2 className="text-xl font-semibold text-white">{title}</h2>
      <Link 
        to={viewMoreLink}
        className="text-sm text-blue-400 hover:text-blue-300"
      >
        View more
      </Link>
    </div>
    {children}
  </div>
);

const ListItem = ({ title, subtitle, status }: { 
  title: string; 
  subtitle?: string; 
  status?: 'active' | 'inactive' | 'error' 
}) => {
  const statusColors = {
    active: 'bg-green-500',
    inactive: 'bg-gray-500',
    error: 'bg-red-500',
  };

  return (
    <div className="border-t border-gray-700 py-3 flex items-center justify-between">
      <div>
        <h3 className="text-sm font-medium text-gray-200">{title}</h3>
        {subtitle && (
          <p className="text-xs text-gray-400">{subtitle}</p>
        )}
      </div>
      {status && (
        <div className={`w-2 h-2 rounded-full ${statusColors[status]}`} />
      )}
    </div>
  );
};

export const Overview = () => {
  const { data: apis, isLoading: apisLoading } = useApiDefinitions();
  const { data: components, isLoading: componentsLoading } = useComponents();
  const { data: plugins, isLoading: pluginsLoading } = usePlugins();

  return (
    <div className="space-y-6">
      {/* Grid section */}
      <div className="grid grid-cols-8 gap-6">
        {/* APIs - 3/8 width */}
        <div className="col-span-3">
          <SectionCard title="APIs" viewMoreLink="/api">
            {apisLoading ? (
              <div className="text-gray-400 text-sm">Loading...</div>
            ) : (
              <div className="space-y-1">
                {apis?.slice(0, 5).map((api) => (
                  <ListItem 
                    key={api.id}
                    title={api.id}
                    subtitle={`Version ${api.version}`}
                    status={api.draft ? 'inactive' : 'active'}
                  />
                ))}
                {!apis?.length && (
                  <p className="text-gray-400 text-sm">No APIs defined</p>
                )}
              </div>
            )}
          </SectionCard>
        </div>

        {/* Components - 5/8 width */}
        <div className="col-span-5">
          <SectionCard title="Components" viewMoreLink="/components">
            {componentsLoading ? (
              <div className="text-gray-400 text-sm">Loading...</div>
            ) : (
              <div className="space-y-1">
                {components?.slice(0, 5).map((component) => (
                  <ListItem
                    key={component.versionedComponentId.componentId}
                    title={component.componentName}
                    subtitle={`Version ${component.versionedComponentId.version}`}
                    status="active"
                  />
                ))}
                {!components?.length && (
                  <p className="text-gray-400 text-sm">No components available</p>
                )}
              </div>
            )}
          </SectionCard>
        </div>
      </div>

      {/* Plugins section - Full width */}
      <SectionCard title="Plugins" viewMoreLink="/plugins">
        {pluginsLoading ? (
          <div className="text-gray-400 text-sm">Loading...</div>
        ) : (
          <div className="space-y-1">
            {plugins?.slice(0, 5).map((plugin) => (
              <ListItem
                key={`${plugin.name}-${plugin.version}`}
                title={plugin.name}
                subtitle={`Version ${plugin.version}`}
                status={plugin.scope.type === 'Global' ? 'active' : 'inactive'}
              />
            ))}
            {!plugins?.length && (
              <p className="text-gray-400 text-sm">No plugins installed</p>
            )}
          </div>
        )}
      </SectionCard>
    </div>
  );
};