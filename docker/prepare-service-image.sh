#!/bin/sh
# Prepares a Golem service image to run its service as uid and gid 65534. The Dockerfile of each
# service runs this script from a bind mount, so the script is not part of the image.
#
# The image gives that id to its own user and group golem, in place of the base image's user and
# group with that id (nobody and nogroup), so that the user has a home for caches such as the
# Wasmtime cache. The group keeps its other members. The user owns /data, where the default
# configuration keeps local data, /data/blob_storage, and each directory given as an argument.
#
# No program in the image gains privileges when it runs: the script clears every setuid and setgid
# bit, so a process in the container cannot become root through a program of the base image.
set -eu

if user=$(getent passwd 65534); then userdel "${user%%:*}"; fi
if group=$(getent group 65534); then
    groupmod --new-name golem "${group%%:*}"
else
    groupadd --gid 65534 golem
fi
useradd --uid 65534 --gid 65534 --home-dir /home/golem --create-home --shell /usr/sbin/nologin golem
install --directory --owner 65534 --group 65534 --mode 0750 /data /data/blob_storage "$@"

find / -xdev -perm /6000 -type f -exec chmod a-s {} +
