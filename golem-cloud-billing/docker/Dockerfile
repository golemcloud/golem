# Build context: the golem-cloud-billing directory.

# The bundle is plain JavaScript, so it is built once on the build platform for every target platform.
FROM --platform=$BUILDPLATFORM node:24-trixie-slim AS build

WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY rolldown.config.ts ./
COPY src ./src
RUN pnpm build

# Distroless: Node.js and its runtime libraries, without a shell or a package manager. The bundle
# holds every dependency, so the image has no node_modules.
FROM gcr.io/distroless/nodejs24-debian13

WORKDIR /app
COPY --from=build /app/dist ./

ENV NODE_ENV=production
ENV NODE_OPTIONS=--enable-source-maps
USER 65534:65534
EXPOSE 8080

CMD ["main.mjs"]
