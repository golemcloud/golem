# Local logging tools

This folder contains tools for helping with local debugging with logs.

## lnav format

The **lnav** folder contains a [lnav log format definition](./lnav/golem-json.json)
that works for **golem JSON logs**.

Copy this file to e.g. `.lnav/formats/installed`, after that
lnav should automatically recognize golem JSON logs.

## Local elastic environment

Use `cargo make elastic-up` to _start_ the **elastic**, **kibana** and **filebeat** docker containers.
Filebeat is configured to load logs from `./logs/*.logs`.

On the first start it might take some time until _kibana_ is booted and until
provisioning finishes, but after that the
preconfigured [log view](http://localhost:5601/app/discover#/view/a6528e3b-703e-4b11-839c-8436f7009e61)
should be available.

For _credentials_ see the [env config](./elastic/.env).

For _stopping_ the _elastic stack_ use `cargo make elastic-stop`.
