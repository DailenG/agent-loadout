//! Effective-view goldens (SPEC 6.4) for the Tier A clients over one
//! project (`fixtures/projects/effective`): a system server, a project
//! override with `from` shadowing it, a project-only server, a suppression,
//! a private-only entry, foreign entries and omp's compatibility sources.

mod support;

use std::path::{Path, PathBuf};

use agent_loadout_core::adapters::adapter;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::layering::{EffectiveView, LOCAL_FILE, effective};
use serde_json::Value;
use support::{Fx, host_os, stdio};

const ALL: &[&str] = &["claude-code", "omp", "codex", "claude-desktop"];

/// The applied fixture project: `github` and `slack` enabled for every Tier
/// A client but Pi, foreign entries in `~/.claude.json` and `<P>/.mcp.json`,
/// the project known and trusted.
fn applied() -> (Fx, PathBuf) {
    let fx = Fx::new();
    fx.put_server(stdio("github", ALL, &["-y", "github-mcp"]));
    fx.put_server(stdio("slack", ALL, &["-y", "slack-mcp"]));
    fx.write(
        &fx.claude(),
        r#"{ "mcpServers": { "legacy": { "command": "legacy-mcp" } } }"#,
    );
    let root = fx.project_from("effective", "effective");
    fx.write(
        &root.join(".mcp.json"),
        r#"{ "mcpServers": { "scratch": { "command": "scratch-mcp" } } }"#,
    );
    fx.know_projects(&[("effective-e7f8", &root)]);
    fx.trust(&root);
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    (fx, root)
}

fn view(fx: &Fx, folder: &Path, client: &str) -> EffectiveView {
    let store = fx.store();
    effective(Some(&store), &fx.ax(), folder, Some(&[client.to_owned()])).unwrap()
}

/// The view as JSON text, with the temp project folder as `<P>` and Claude
/// Desktop's per-OS config file as `<claude-desktop>`.
fn golden(fx: &Fx, root: &Path, view: &EffectiveView) -> String {
    let mut text = serde_json::to_string_pretty(view).unwrap();
    let raw = root.to_string_lossy().into_owned();
    let escaped = serde_json::to_string(&raw).unwrap();
    text = text.replace(&escaped[1..escaped.len() - 1], "<P>");
    text = text.replace(&raw, "<P>");
    if let Some(spec) = adapter("claude-desktop")
        && let Ok(targets) = spec.mcp_targets(
            &fx.env,
            &fx.fs,
            agent_loadout_core::model::applied::Scope::User,
            None,
            &Default::default(),
        )
    {
        let file = placeholder_path(&fx.env, &targets[0].file, None);
        text = text.replace(&file, "<claude-desktop>");
    }
    text
}

fn entry<'a>(doc: &'a Value, client: &str, name: &str) -> &'a Value {
    doc["clients"][client]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no {client}/{name} in {doc:#}"))
}

#[test]
fn claude_code() {
    let (fx, root) = applied();
    // From a folder below the project root: the nearest manifest wins.
    let folder = root.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let view = view(&fx, &folder, "claude-code");
    let doc = serde_json::to_value(&view).unwrap();
    assert_eq!(doc["project"]["id"], "effective-e7f8");

    let github = entry(&doc, "claude-code", "github");
    assert_eq!(github["winner"]["layer"], "project");
    assert_eq!(github["winner"]["source"], "store:github+project");
    assert_eq!(github["shadowed"][0]["layer"], "system");
    // `<P>/.mcp.json` is also Copilot's workspace file (SPEC 6.5).
    assert_eq!(
        github["also_visible_to"],
        serde_json::json!(["copilot", "omp"])
    );
    let slack = entry(&doc, "claude-code", "slack");
    assert_eq!(slack["loaded"], false);
    assert_eq!(
        entry(&doc, "claude-code", "notes")["winner"]["layer"],
        "project-local"
    );
    assert_eq!(
        entry(&doc, "claude-code", "legacy")["winner"]["layer"],
        "foreign:~/.claude.json"
    );
    insta::assert_snapshot!("claude_code", golden(&fx, &root, &view));
}

#[test]
fn omp() {
    let (fx, root) = applied();
    let view = view(&fx, &root, "omp");
    let doc = serde_json::to_value(&view).unwrap();
    let docs = entry(&doc, "omp", "docs");
    assert_eq!(docs["winner"]["layer"], "project");
    assert_eq!(docs["warnings"][0]["code"], "duplicate_via_compat");
    assert_eq!(entry(&doc, "omp", "slack")["loaded"], false);
    assert_eq!(
        entry(&doc, "omp", "scratch")["winner"]["layer"],
        "client-compat:<P>/.mcp.json"
    );
    insta::assert_snapshot!("omp", golden(&fx, &root, &view));
}

#[test]
fn codex() {
    let (fx, root) = applied();
    let view = view(&fx, &root, "codex");
    let doc = serde_json::to_value(&view).unwrap();
    assert_eq!(entry(&doc, "codex", "slack")["loaded"], false);
    assert_eq!(
        entry(&doc, "codex", "slack")["winner"]["source"],
        "store:slack+project"
    );
    assert_eq!(
        entry(&doc, "codex", "docs")["winner"]["source"],
        "project:docs"
    );
    insta::assert_snapshot!("codex", golden(&fx, &root, &view));
}

#[test]
fn claude_desktop() {
    let (fx, root) = applied();
    let spec = adapter("claude-desktop").unwrap();
    if !spec.mcp.as_ref().unwrap().oses.contains(&host_os()) {
        return;
    }
    let view = view(&fx, &root, "claude-desktop");
    let doc = serde_json::to_value(&view).unwrap();
    // No project layer: the system entries win, and the suppression cannot
    // be expressed.
    assert_eq!(
        entry(&doc, "claude-desktop", "github")["winner"]["layer"],
        "system"
    );
    let slack = entry(&doc, "claude-desktop", "slack");
    assert_eq!(slack["loaded"], true);
    assert_eq!(slack["warnings"][0]["code"], "cannot_suppress_in_project");
    insta::assert_snapshot!("claude_desktop", golden(&fx, &root, &view));
}

#[test]
fn untrusted_project() {
    let (fx, root) = applied();
    // Edited after it was trusted: nothing of the project is written, and
    // every project entry says why.
    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\n\n[servers.notes]\ntransport = \"stdio\"\ncommand = \"notes-mcp\"\n\
         clients = [\"claude-code\", \"omp\"]\n\n[servers.extra]\ntransport = \"stdio\"\n\
         command = \"extra-mcp\"\nclients = [\"claude-code\"]\n",
    );
    let view = view(&fx, &root, "claude-code");
    let doc = serde_json::to_value(&view).unwrap();
    let extra = entry(&doc, "claude-code", "extra");
    assert_eq!(extra["loaded"], false);
    assert_eq!(extra["status"], "blocked");
    assert_eq!(extra["warnings"][0]["code"], "trust_required");
    assert_eq!(
        entry(&doc, "claude-code", "github")["warnings"][0]["code"],
        "trust_required"
    );
    insta::assert_snapshot!("untrusted", golden(&fx, &root, &view));
}

#[test]
fn folder_without_project() {
    let (fx, _root) = applied();
    // Inside the home folder: discovery stops there.
    let folder = fx.th.home().join("elsewhere");
    std::fs::create_dir_all(&folder).unwrap();
    let view = view(&fx, &folder, "claude-code");
    assert!(view.project.is_none());
    let names: Vec<&str> = view.clients["claude-code"]
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["github", "legacy", "slack"]);
    assert!(
        view.clients["claude-code"]
            .iter()
            .all(|entry| entry.loaded && entry.shadowed.is_empty()),
        "{view:#?}"
    );
}
