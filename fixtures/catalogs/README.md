Catalog fixtures (#26). `registry/`: recorded MCP Registry answers. `tap/`: a git tap laid out like
anthropics/skills (skill folders under `skills/`, a `template/` and `spec/` that are not skills, and a
`.claude-plugin/marketplace.json`) whose marketplace lists a plugin of every source kind: relative (one with
skills only, one with an `.mcp.json`, a skill, hooks and commands), `github` with a sha, `url` with a ref,
`git-subdir`, and the refused `npm`, `archive` and `command`, plus a path escaping the tap and a bad name. Its
marketplace name is reserved for Anthropic, so a tap of it elsewhere carries a warning. Tests commit it into a
local bare repository; nothing reaches the network.
