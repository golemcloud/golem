FROM envoyproxy/envoy:v1.36.13

COPY golem-router/docker/golem-router-entrypoint.sh /usr/local/bin/golem-router-entrypoint

# Runs Envoy as uid and gid 65534, like the images of the services; see the script.
RUN --mount=type=bind,source=docker/prepare-service-image.sh,target=/tmp/prepare-service-image.sh \
    sh /tmp/prepare-service-image.sh

ENV HOME=/home/golem
USER 65534:65534

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/golem-router-entrypoint"]
CMD []
