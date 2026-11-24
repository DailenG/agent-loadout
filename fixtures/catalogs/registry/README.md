Recorded MCP Registry answers (`/v0.1/servers`, `/v0.1/servers/<name>/versions/<version>`) for the catalog tests
(#26): a search page with an older and the latest version of one server (2025-12-11 schema with `icons`, remote
`variables` and headers, publisher-provided `_meta`), a deprecated entry, a deleted one, a bare item in the first
snake_case schema, and an entry a registry that ignores `search` would also return. Shapes follow the registry's
openapi.json; names and URLs are made up.
