# Claude Desktop: gate V1, Windows

Both installer kinds are present on this machine. Only the read-only check
ran; the rest change the real app's config and wait for Windows Sandbox or
the maintainer's OK.

| # | Result | Evidence |
|---|---|---|
| CD-1 | Not run | Real `%APPDATA%\Claude\claude_desktop_config.json`. |
| CD-2 | Not run | Real MSIX config. Package folder name observed: `Claude_pzs8sxrjxfjjc` (matches `Claude_*`). |
| CD-3 | Pass, note | Classic install: `HKCU\...\Uninstall\AnthropicClaude`, `DisplayName` = `Claude`. MSIX: package `Claude`, family `Claude_pzs8sxrjxfjjc`, version 6.28899.2.0. The same key also holds `Claude Code` (WinGet), so the future registry probe must match `DisplayName` exactly, not as a substring. (The CLI has no registry probe yet, `NoRegistry`.) |
| CD-4 | Not run | |
| CD-5 | Not run | |
| CD-6 | Not run | |
| CD-7 | Not run | macOS. |
