# Exit codes, error codes and entry states

## Exit codes

| Code | Meaning | Tell the user |
|---|---|---|
| 0 | OK | |
| 1 | Error; read `error.code` | See the codes below |
| 2 | Usage error | Your command line was wrong; check `loadout <command> --help` |
| 3 | Pending changes or drift (`status --check`, `diff --exit-code`) | An apply would change client files |
| 4 | Entries need a decision (conflict, collision, foreign entry) | They choose in the app's apply drawer, or with `loadout apply` |
| 5 | Missing secrets block entries | Which aliases, and `loadout secrets set <alias>` |
| 6 | Timed out waiting for the store lock | Another Loadout run is busy; try again shortly |
| 7 | A project manifest is untrusted or changed | They review and run `loadout project trust <path>` |
| 8 | The built-in scan blocked an install or update | They review the findings; only they may override it |

## Error codes

With `--json`, a failure is `{"schema": 1, "error": {"code", "message",
"path"}}`. Warnings inside documents use the same `code` and `message`.

| Code | Meaning |
|---|---|
| `not_found` | No such server, skill, project or commit; or there is no store yet |
| `already_exists` | The id is taken |
| `invalid_value`, `invalid_id`, `invalid_path` | An argument or a store file has a bad value |
| `parse_error`, `unsupported_schema` | A store or client file does not parse, or is from a newer Loadout |
| `unknown_client` | Not a client id Loadout knows |
| `scope_not_supported`, `not_supported_on_os` | The client has no file for that scope or OS |
| `unsupported_transport`, `unsupported_field` | The client cannot run the server as defined; the target is blocked |
| `trust_required` | A project manifest must be trusted before Loadout writes its targets |
| `source_trust_required`, `source_blocked` | A skill or server source is not trusted, or is blocked |
| `scan_blocked` | The built-in scan found dangerous content |
| `missing_secret`, `secret_locked`, `provider_unavailable` | A secret cannot be resolved on this machine |
| `read_only_target` | Loadout cannot write that client file or folder (permissions) |
| `client_enable_required` | The client needs a setting turned on before it loads servers from that file |
| `restart_required` | The client reads the file only at startup |
| `locked` | Another Loadout run holds the store lock |
| `merge_conflict`, `remote_unreachable`, `remote_auth_failed` | Remote sync problems |
| `catalogs_off` | Online catalogs are off on this machine; nothing was contacted. Hand the user `loadout catalog online on` |
| `catalog_unreachable` | A catalog did not answer (timeout, HTTP error, rate limit, bad data); others may have |

## Entry states (`status`, `apply --dry-run`)

| State | Meaning |
|---|---|
| `synced` | The client file matches the store |
| `pending_create`, `pending_update`, `pending_remove` | An apply would write this |
| `drifted` | Someone edited a Loadout entry in the client file |
| `missing` | A Loadout entry was removed from the client file |
| `conflict` | Both the store and the file changed |
| `blocked` | The entry cannot be written (see its `problem`) |
| `foreign` | An entry Loadout does not manage; never touched, offered for import |
| `collision` | An entry Loadout does not manage has the name of an entry the store wants |

`unresolved: true` on an entry means it waits for the user's decision.
