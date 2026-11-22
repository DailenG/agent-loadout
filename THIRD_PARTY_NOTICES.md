# Third-party notices

Loadout (agent-loadout) is MIT licensed (see `LICENSE`). Portions are derived
from the projects below. Every derived source file carries this header:

```
// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
```

## SkillDock

- Origin: `DailenG/skilldock@181b52b` (https://github.com/DailenG/skilldock)
- License: MIT
- Copyright (c) 2026 wanghuan

### Reused files

| File in this repository | Origin | License | Copyright |
|---|---|---|---|
| `crates/agent-loadout-core/src/secrets/classify.rs` (`SENSITIVE_ARGUMENT_NAMES` and the argument-name rule of `is_sensitive_argument_name`, SPEC 11.7) | `src-tauri/src/mcp_manager.rs` (`SENSITIVE_ARGUMENT_NAMES`, `is_sensitive_name`) | MIT | Copyright (c) 2026 wanghuan |
| `src/styles/tokens.css` (verbatim, plus a marked Loadout section at the end; the dark pre-paint color comes from SkillDock's inline `index.html` style) | `src/styles/tokens.css`, `index.html` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/App.tsx` (shell layout: grid, sidebar, brand block, nav and its icons, header, window chrome, route error boundary) | `src/app/App.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/AppSelect.tsx` (verbatim) | `src/app/components/AppSelect.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/AppTooltip.tsx` (verbatim) | `src/app/components/AppTooltip.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/BatchActions.tsx` (verbatim) | `src/app/components/BatchActions.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/SearchFieldIcon.tsx` (verbatim) | `src/app/components/SearchFieldIcon.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/ToolbarGoInstallButton.tsx` | `src/app/components/ToolbarGoInstallButton.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/ListGridViewToggle.tsx` | `src/features/skills/components/ListGridViewToggle.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/notifications.tsx` | `src/app/notifications.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/app/components/SettingsGroup.tsx` (settings group, form row and theme picker markup, `ThemeIcon`) | `src/app/routes/settings.tsx` | MIT | Copyright (c) 2026 wanghuan |
| `src/i18n/en.ts` (shell strings of the `en` catalog) | `src/app/i18n.tsx` | MIT | Copyright (c) 2026 wanghuan |

Each further port adds a row here with its origin path at
`DailenG/skilldock@181b52b`.

### License text

```
MIT License

Copyright (c) 2026 wanghuan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
