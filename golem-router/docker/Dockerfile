FROM nginx:1.29

# Only the GOLEM_ROUTER_ variables are replaced in the template, so the nginx variables stay.
ENV NGINX_ENVSUBST_FILTER=^GOLEM_ROUTER_
# The default resolver is the name servers of /etc/resolv.conf.
ENV NGINX_ENTRYPOINT_LOCAL_RESOLVERS=1

COPY golem-router/docker/16-golem-router.envsh /docker-entrypoint.d/16-golem-router.envsh
COPY golem-router/golem-router.conf.template /etc/nginx/templates/default.conf.template

EXPOSE 80
