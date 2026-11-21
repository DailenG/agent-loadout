# Secrets: gate V1, Windows

Method in [README](README.md).

| # | Result | Evidence |
|---|---|---|
| SE-1 | Not run | Real 1Password CLI. |
| SE-2 | Not run | Real 1Password item and desktop app states. |
| SE-3 | Not run | Real 1Password Environment. |
| SE-4 | Not run | Arch with the AUR CLI. |
| SE-5 | Not run | Arch without the CLI. |
| SE-6 | Not run | Real Windows Credential Manager. (The debug build's test keyring file was used for the delivery checks instead.) |
| SE-7 | Pass | `server add` with a description holding `https://user:password@...` and one holding a hand-built `ghp_` token: both refused with `leak_detected`, naming the file and line, neither value echoed. |
