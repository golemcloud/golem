# OpenAPI validation schemas

Unmodified schemas published by the OpenAPI Initiative:

- `schema.json`: https://spec.openapis.org/oas/3.1/schema/2025-11-23
- `dialect.json`: https://spec.openapis.org/oas/3.1/dialect/base
- `meta.json`: https://spec.openapis.org/oas/3.1/meta/base

Embedded for offline structural validation. Golem's semantic validation further
restricts this schema to its supported provider-document subset. Provider
references are never retrieved or compiled as executable validation schemas.

The validator adjusts Link parameter values to allow arbitrary JSON literals,
as required by OpenAPI 3.1.0 section 4.8.20. Format keywords remain annotations.

Copyright The Linux Foundation. Distributed under the Apache License 2.0;
see `LICENSE` (from https://github.com/OAI/OpenAPI-Specification).
