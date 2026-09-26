FROM bash@sha256:bfe38ab74faa887cdbe5849dad0572fde3a5c75b99ea554890de7e52c7de8f6d
RUN apk add --no-cache coreutils grep sed jq diffutils patch findutils file curl wget \
 && ln -sf /usr/local/bin/bash /bin/sh
ENV LC_ALL=C.UTF-8
