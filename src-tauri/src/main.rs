//! Loadout desktop app (SPEC 4.1, 13). Commands are thin wrappers over
//! `agent-loadout-core`; their TypeScript bindings (`src/ipc/bindings.ts`)
//! are generated from this crate by `tauri-specta` (`npm run bindings`).

// No console window behind release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use agent_loadout_core::brand;
use tauri_specta::{Builder, ErrorHandlingMode, collect_commands};

/// Every command and constant the frontend sees. Commands that can fail
/// resolve to `{ status: "ok", data } | { status: "error", error: IpcError }`.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app_info,
            commands::list_scopes,
            commands::data_home,
            commands::servers::list_servers,
            commands::servers::get_server,
            commands::servers::save_server,
            commands::servers::save_server_toml,
            commands::servers::server_draft_to_toml,
            commands::servers::server_toml_to_draft,
            commands::servers::remove_servers,
            commands::servers::set_server_clients,
            commands::servers::preview_server,
            commands::servers::convert_snippet,
            commands::servers::list_secret_aliases,
            commands::apply::pending_plan,
            commands::apply::run_apply,
            commands::apply::list_applied_runs,
            commands::apply::undo_apply,
            commands::tools::list_tools,
            commands::tools::reveal_tool_folder,
            commands::projects::list_projects,
            commands::projects::pick_project_folder,
            commands::projects::add_project,
            commands::projects::init_project,
            commands::projects::review_project,
            commands::projects::trust_project,
            commands::projects::remove_project,
            commands::projects::apply_project_files,
            commands::projects::reveal_project_folder,
            commands::effective::effective_matrix,
            commands::resolve::resolver_detail,
            commands::resolve::resolve_entry,
        ])
        .error_handling(ErrorHandlingMode::Result)
        // One TypeScript type per Rust type: the IPC types serialize and
        // deserialize alike, so the `_Serialize`/`_Deserialize` split adds
        // nothing.
        .disable_serde_phases()
        .constant("PRODUCT_NAME", brand::PRODUCT_NAME)
        .constant("APP_ID", brand::APP_ID)
        .constant("REPO", brand::REPO)
        .constant("HOME_ENV_VAR", brand::HOME_ENV_VAR)
        .constant("BIN_NAME", brand::BIN_NAME)
}

fn main() {
    let builder = specta_builder();
    tauri::Builder::default()
        .manage(commands::AppState::from_process())
        // The folder picker of Add folder runs from Rust (the path stays in
        // the backend), so the frontend needs no dialog permission.
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the Loadout window failed to start");
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use specta_typescript::Typescript;

    use super::*;

    const HEADER: &str =
        "// Source: src-tauri/src/{main,commands}.rs. Regenerate with `npm run bindings`.";

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .to_path_buf()
    }

    fn normalized(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    fn generated_bindings() -> String {
        let scratch = std::env::temp_dir().join(format!(
            "loadout-bindings-{}-{}.ts",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        specta_builder()
            .export(Typescript::default().header(HEADER), &scratch)
            .expect("bindings export");
        let generated = normalized(&std::fs::read_to_string(&scratch).expect("read export"));
        let _ = std::fs::remove_file(&scratch);
        generated
    }

    fn committed_bindings_path() -> PathBuf {
        repo_root().join("src/ipc/bindings.ts")
    }

    /// `src/ipc/bindings.ts` matches the commands.
    #[test]
    fn bindings_are_current() {
        let committed =
            normalized(&std::fs::read_to_string(committed_bindings_path()).unwrap_or_default());
        assert!(
            committed == generated_bindings(),
            "src/ipc/bindings.ts is stale; run `npm run bindings`"
        );
    }

    /// Rewrites `src/ipc/bindings.ts` (`npm run bindings` runs this ignored
    /// test).
    #[test]
    #[ignore = "writes src/ipc/bindings.ts; run through `npm run bindings`"]
    fn write_bindings() {
        std::fs::write(committed_bindings_path(), generated_bindings()).expect("write bindings");
    }

    fn read_config(name: &str) -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
        serde_json::from_str(&std::fs::read_to_string(path).expect("read config"))
            .expect("parse config")
    }

    /// `tauri.conf.json` repeats brand strings Tauri reads before any Rust
    /// runs; they must equal `brand.rs`.
    #[test]
    fn tauri_config_uses_the_brand_strings() {
        let config = read_config("tauri.conf.json");
        assert_eq!(config["productName"], brand::PRODUCT_NAME);
        assert_eq!(config["identifier"], brand::APP_ID);
        assert_eq!(config["mainBinaryName"], "agent-loadout-app");
        assert_eq!(config["app"]["windows"][0]["title"], brand::PRODUCT_NAME);
        let csp = config["app"]["security"]["csp"]
            .as_str()
            .expect("CSP is set (SPEC 4.3)");
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(!csp.contains("unsafe-eval"), "{csp}");
    }

    /// The platform files replace `app.windows` whole (JSON Merge Patch), so
    /// each window must equal the base one apart from its chrome keys
    /// (SPEC 13.1), title included.
    #[test]
    fn platform_configs_only_change_the_window_chrome() {
        let base = read_config("tauri.conf.json")["app"]["windows"].clone();
        for (name, chrome) in [
            ("tauri.windows.conf.json", &["decorations", "shadow"][..]),
            (
                "tauri.macos.conf.json",
                &["hiddenTitle", "titleBarStyle"][..],
            ),
        ] {
            let mut windows = read_config(name)["app"]["windows"].clone();
            let entries = windows.as_array_mut().expect("windows array");
            assert_eq!(entries.len(), 1, "{name}");
            assert_eq!(entries[0]["title"], brand::PRODUCT_NAME, "{name}");
            let window = entries[0].as_object_mut().expect("window object");
            for key in chrome {
                assert!(window.remove(*key).is_some(), "{name} sets {key}");
            }
            assert_eq!(
                windows, base,
                "{name} differs from tauri.conf.json beyond {chrome:?}"
            );
        }
    }
}
