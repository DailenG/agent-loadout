//! Project scope apply (SPEC 6, 8): known projects of this machine's
//! overlay, placement of shared and private entries, the private-file
//! rules of 6.5, suppressions, trust, selection and clean-up.

mod support;

use std::path::Path;

use agent_loadout_core::ErrorCode;
use agent_loadout_core::adapters::location::project_key;
use agent_loadout_core::apply::ledger::EntryKind;
use agent_loadout_core::apply::{
    EntryState, PlanOptions, ProjectSelection, ScopeFilter, TargetPlan,
};
use agent_loadout_core::layering::{LOCAL_FILE, SHARED_FILE};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::{AppliedRecord, Scope};
use serde_json::Value;
use support::{Fx, entry, git_commit, git_init, host_os, json_entry, stdio};

const SECRETS: &str =
    "schema = 1\n\n[secrets.DOCS_TOKEN]\nref = \"keyring://loadout/DOCS_TOKEN\"\n";

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// `projects["<key of root>"]` in `~/.claude.json`.
fn claude_project(fx: &Fx, root: &Path) -> Value {
    let claude = json(&fx.read(&fx.claude()));
    claude["projects"][project_key(host_os(), root)].clone()
}

fn target<'a>(plan: &'a agent_loadout_core::apply::Plan, file: &Path) -> &'a TargetPlan {
    plan.targets
        .iter()
        .find(|target| target.file == file)
        .unwrap_or_else(|| panic!("no target {} in {plan:#?}", file.display()))
}

fn exclude_lines(repo: &git2::Repository) -> Vec<String> {
    std::fs::read_to_string(repo.path().join("info").join("exclude"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn shared_and_private_entries_land_in_their_files() {
    let fx = Fx::new();
    fx.edit("secrets", |tx| tx.write("secrets.toml", SECRETS));
    fx.keyring
        .insert("loadout", "DOCS_TOKEN", "fake-docs-token-0001");
    let root = fx.project_from("web-app", "web-app");
    let repo = git_init(&root);
    fx.know_projects(&[("web-app-a1b2", &root)]);
    fx.trust(&root);

    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert!(
        report
            .targets
            .iter()
            .all(|target| target.project_id.as_deref() == Some("web-app-a1b2")),
        "{report:#?}"
    );

    // Shared-only: the project file, started through the bare launcher.
    let docs = json_entry(&fx.read(&root.join(".mcp.json")), "docs").unwrap();
    assert_eq!(docs["command"], "loadout");
    assert_eq!(docs["args"][0], "exec");

    // Local: Claude Code's private project entry, with `${project}` filled.
    let notes = &claude_project(&fx, &root)["mcpServers"]["notes"];
    assert_eq!(notes["command"], "notes-mcp");
    assert_eq!(notes["args"][1], root.to_string_lossy().as_ref());
    assert!(json(&fx.read(&fx.claude())).get("mcpServers").is_none());

    // omp has no private target: its project file, hidden from git.
    let omp_file = root.join(".omp").join("mcp.json");
    assert!(json_entry(&fx.read(&omp_file), "notes").is_some());
    let exclude = exclude_lines(&repo);
    assert!(
        exclude.contains(&"/.omp/mcp.json".to_owned()),
        "{exclude:?}"
    );
    assert!(!exclude.contains(&"/.mcp.json".to_owned()), "{exclude:?}");

    let ledger = fx.ledger();
    assert_eq!(ledger.entries.len(), 3, "{ledger:#?}");
    for row in &ledger.entries {
        assert_eq!(row.project_id.as_deref(), Some("web-app-a1b2"));
        assert_eq!(row.kind, EntryKind::McpEntry);
    }
    let scope_of = |key: &str, client: &str| {
        ledger
            .entries
            .iter()
            .find(|row| row.key == key && row.client == client)
            .unwrap()
            .scope
    };
    assert_eq!(scope_of("docs", "claude-code"), Scope::Project);
    assert_eq!(scope_of("notes", "claude-code"), Scope::ProjectLocal);
    assert_eq!(scope_of("notes", "omp"), Scope::Project);

    // applied.toml records the project and placeholder paths.
    let machine = fx.machine_id();
    let text = fx
        .store()
        .read_to_string(&format!("machines/{machine}.applied.toml"))
        .unwrap()
        .unwrap();
    let record = TomlDocument::<AppliedRecord>::parse(&text)
        .unwrap()
        .into_value();
    assert!(record.validate(Some(&machine)).is_valid(), "{text}");
    let files: Vec<(&str, Option<&str>)> = record
        .targets
        .iter()
        .map(|target| (target.file.as_str(), target.project_id.as_deref()))
        .collect();
    assert!(
        files.contains(&("<P>/.mcp.json", Some("web-app-a1b2"))),
        "{files:?}"
    );
    assert!(
        files.contains(&("<P>/.omp/mcp.json", Some("web-app-a1b2"))),
        "{files:?}"
    );
    assert!(
        files.contains(&("~/.claude.json", Some("web-app-a1b2"))),
        "{files:?}"
    );

    // Applying again changes nothing.
    let (again, commit) = fx.apply();
    assert_eq!(again.writes(), 0, "{again:#?}");
    assert!(commit.is_none());
    assert!(!fx.plan().has_work());
    assert_eq!(exclude_lines(&repo), exclude);

    // A project this machine no longer knows: its owned entries go.
    fx.know_projects(&[]);
    let (report, _) = fx.apply();
    assert_eq!(report.writes(), 3, "{report:#?}");
    assert!(json_entry(&fx.read(&root.join(".mcp.json")), "docs").is_none());
    assert!(json_entry(&fx.read(&omp_file), "notes").is_none());
    assert!(
        claude_project(&fx, &root)["mcpServers"]
            .get("notes")
            .is_none()
    );
    assert!(fx.ledger().entries.is_empty());
}

#[test]
fn private_entries_never_go_to_tracked_files() {
    let fx = Fx::new();
    let root = fx.project_from("tracked", "tracked");
    let repo = git_init(&root);
    let omp_file = root.join(".omp").join("mcp.json");
    let codex_file = root.join(".codex").join("config.toml");
    fx.write(&omp_file, "{\n  \"mcpServers\": {}\n}\n");
    fx.write(&codex_file, "# shared Codex settings\n");
    git_commit(&repo, ".omp/mcp.json");
    git_commit(&repo, ".codex/config.toml");
    fx.know_projects(&[("tracked-e5f6", &root)]);
    fx.trust(&root);

    let plan = fx.plan();
    for file in [&omp_file, &codex_file] {
        let notes = entry(&plan, file, "notes");
        assert_eq!(notes.state, EntryState::Blocked, "{notes:#?}");
        assert_eq!(
            notes.problem.as_ref().unwrap().code(),
            ErrorCode::PrivateInTrackedFile
        );
        assert!(target(&plan, file).after.is_none());
    }
    let (report, _) = fx.apply();
    assert_eq!(report.writes(), 0, "{report:#?}");
    assert_eq!(fx.read(&omp_file), "{\n  \"mcpServers\": {}\n}\n");
    assert_eq!(fx.read(&codex_file), "# shared Codex settings\n");
    assert!(!exclude_lines(&repo).iter().any(|line| line.contains("omp")));
}

#[test]
fn suppressions_use_each_clients_mechanism() {
    let fx = Fx::new();
    fx.put_server(stdio(
        "slack",
        &["claude-code", "omp", "codex"],
        &["-y", "slack-mcp"],
    ));
    let root = fx.project_from("quiet", "quiet");
    fx.know_projects(&[("quiet-c3d4", &root)]);
    fx.trust(&root);
    fx.apply();

    // Claude Code: an owned member of the project's disabled list.
    assert_eq!(
        claude_project(&fx, &root)["disabledMcpServers"],
        serde_json::json!(["slack"])
    );
    let ledger = fx.ledger();
    let member = ledger
        .entries
        .iter()
        .find(|row| row.kind == EntryKind::ListMember)
        .unwrap();
    assert_eq!(
        (member.client.as_str(), member.key.as_str(), member.scope),
        ("claude-code", "slack", Scope::ProjectLocal)
    );
    assert_eq!(member.project_id.as_deref(), Some("quiet-c3d4"));

    // omp replaces the user entry: the whole entry, disabled.
    let omp_file = root.join(".omp").join("mcp.json");
    let slack = json_entry(&fx.read(&omp_file), "slack").unwrap();
    assert_eq!(slack["enabled"], false);
    assert!(slack.get("command").is_some());

    // Codex merges: `enabled = false` alone.
    let codex_file = root.join(".codex").join("config.toml");
    let codex: toml::Table = toml::from_str(&fx.read(&codex_file)).unwrap();
    assert_eq!(
        codex["mcp_servers"]["slack"],
        toml::Value::Table(toml::toml! { enabled = false })
    );

    // The system entries stay.
    assert!(json_entry(&fx.read(&fx.claude()), "slack").is_some());
    let (again, _) = fx.apply();
    assert_eq!(again.writes(), 0, "{again:#?}");

    // Dropping the suppression undoes it.
    fx.manifest(&root, SHARED_FILE, "schema = 1\nid = \"quiet-c3d4\"\n");
    fx.trust(&root);
    let (report, _) = fx.apply();
    assert_eq!(report.writes(), 3, "{report:#?}");
    assert!(
        claude_project(&fx, &root)
            .get("disabledMcpServers")
            .is_none_or(|list| list == &serde_json::json!([]))
    );
    assert!(json_entry(&fx.read(&omp_file), "slack").is_none());
    assert!(!fx.read(&codex_file).contains("slack"));
    assert!(
        fx.ledger()
            .entries
            .iter()
            .all(|row| row.project_id.is_none())
    );
}

#[test]
fn untrusted_and_changed_manifests_block_the_project_only() {
    let fx = Fx::new();
    fx.put_server(stdio("fetch", &["claude-code"], &["-y", "fetch-mcp"]));
    let root = fx.th.projects().join("trust-me");
    let manifest = "schema = 1\nid = \"trust-me-a1b2\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
                    [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n";
    fx.manifest(&root, SHARED_FILE, manifest);
    fx.know_projects(&[("trust-me-a1b2", &root)]);
    let mcp = root.join(".mcp.json");

    let plan = fx.plan();
    let blocked = target(&plan, &mcp);
    assert_eq!(
        blocked.problem.as_ref().unwrap().code(),
        ErrorCode::TrustRequired
    );
    assert!(
        blocked
            .problem
            .as_ref()
            .unwrap()
            .message()
            .contains("project trust")
    );
    assert_eq!(entry(&plan, &mcp, "docs").state, EntryState::Blocked);
    assert_eq!(
        entry(&plan, &fx.claude(), "fetch").state,
        EntryState::PendingCreate
    );
    fx.apply();
    assert!(!mcp.exists());
    assert!(json_entry(&fx.read(&fx.claude()), "fetch").is_some());

    // Selection: user files only, or one project only.
    let system = fx.plan_with(&PlanOptions {
        scope: ScopeFilter::System,
        ..PlanOptions::default()
    });
    assert!(system.targets.iter().all(|t| t.project_id.is_none()));
    let only = fx.plan_with(&PlanOptions {
        projects: ProjectSelection::Only(root.clone()),
        ..PlanOptions::default()
    });
    assert!(!only.targets.is_empty());
    assert!(only.targets.iter().all(|t| t.project_id.is_some()));

    fx.trust(&root);
    fx.apply();
    let written = fx.read(&mcp);
    assert!(json_entry(&written, "docs").is_some());

    // A changed manifest needs trust again; nothing is written meanwhile.
    fx.manifest(
        &root,
        SHARED_FILE,
        &format!("{manifest}args = [\"--changed\"]\n"),
    );
    let plan = fx.plan();
    let problem = target(&plan, &mcp).problem.clone().unwrap();
    assert_eq!(problem.code(), ErrorCode::TrustRequired);
    assert!(problem.message().contains("changed"), "{problem}");
    fx.apply();
    assert_eq!(fx.read(&mcp), written);
}

#[test]
fn unknown_template_blocks_only_its_entry() {
    let fx = Fx::new();
    let root = fx.th.projects().join("tmpl");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"tmpl-a1b2\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [servers.ghost]\nfrom = \"nope\"\n\n\
         [servers.real]\ntransport = \"stdio\"\ncommand = \"real-mcp\"\n",
    );
    fx.know_projects(&[("tmpl-a1b2", &root)]);
    fx.trust(&root);
    let plan = fx.plan();
    let mcp = root.join(".mcp.json");
    let ghost = entry(&plan, &mcp, "ghost");
    assert_eq!(ghost.state, EntryState::Blocked);
    assert_eq!(
        ghost.problem.as_ref().unwrap().code(),
        ErrorCode::UnknownTemplate
    );
    assert_eq!(entry(&plan, &mcp, "real").state, EntryState::PendingCreate);
}

#[test]
fn unset_on_a_deep_merging_client_warns() {
    let fx = Fx::new();
    let mut api = stdio("api", &[], &["serve"]);
    api.body.env = Some(
        [("DEBUG", "1"), ("KEEP", "yes")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    );
    fx.put_server(api);
    let root = fx.th.projects().join("unset");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"unset-a1b2\"\n\n[servers.api]\nfrom = \"api\"\nunset = [\"env.DEBUG\"]\n\
         clients = [\"codex\"]\n",
    );
    fx.know_projects(&[("unset-a1b2", &root)]);
    fx.trust(&root);
    let plan = fx.plan();
    let file = root.join(".codex").join("config.toml");
    let codex = target(&plan, &file);
    assert!(
        codex
            .warnings
            .iter()
            .any(|w| w.code() == ErrorCode::UnsetNotExpressible),
        "{codex:#?}"
    );
    let api = entry(&plan, &file, "api");
    assert_eq!(api.state, EntryState::PendingCreate);
    let env = api
        .desired
        .as_ref()
        .unwrap()
        .as_object()
        .unwrap()
        .get("env")
        .unwrap();
    let env = env.as_object().unwrap();
    assert!(env.get("DEBUG").is_none());
    assert!(env.get("KEEP").is_some());
}

#[test]
fn another_spelling_of_the_project_key_warns() {
    let fx = Fx::new();
    let root = fx.th.projects().join("spelled");
    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\nid = \"spelled-a1b2\"\n\n[servers.notes]\ntransport = \"stdio\"\n\
         command = \"notes-mcp\"\nclients = [\"claude-code\"]\n",
    );
    let other = format!("{}/", project_key(host_os(), &root));
    fx.write(
        &fx.claude(),
        &serde_json::json!({ "projects": { other: { "allowedTools": [] } } }).to_string(),
    );
    fx.know_projects(&[("spelled-a1b2", &root)]);
    fx.trust(&root);
    let plan = fx.plan();
    let claude = plan
        .targets
        .iter()
        .find(|t| t.file == fx.claude() && t.scope == Scope::ProjectLocal)
        .unwrap();
    assert!(
        claude
            .warnings
            .iter()
            .any(|w| w.code() == ErrorCode::ProjectKeySpelling),
        "{claude:#?}"
    );
    assert_eq!(
        entry(&plan, &fx.claude(), "notes").state,
        EntryState::PendingCreate
    );
}
