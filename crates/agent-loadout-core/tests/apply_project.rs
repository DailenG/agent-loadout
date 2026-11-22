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

    // Hidden again when nothing is written: the exclude runs whenever the
    // file holds private entries and git does not ignore it.
    let info_exclude = repo.path().join("info").join("exclude");
    let without: String = exclude
        .iter()
        .filter(|line| *line != "/.omp/mcp.json")
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&info_exclude, without).unwrap();
    let (again, _) = fx.apply();
    assert_eq!(again.writes(), 0, "{again:#?}");
    assert!(exclude_lines(&repo).contains(&"/.omp/mcp.json".to_owned()));

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

/// A private-only project with one Claude Code entry, known and trusted.
fn private_project(fx: &Fx, name: &str, id: &str) -> std::path::PathBuf {
    let root = fx.th.projects().join(name);
    fx.manifest(
        &root,
        LOCAL_FILE,
        &format!(
            "schema = 1\nid = \"{id}\"\n\n[servers.notes]\ntransport = \"stdio\"\n\
             command = \"notes-mcp\"\nclients = [\"claude-code\"]\n"
        ),
    );
    fx.know_projects(&[(id, &root)]);
    fx.trust(&root);
    root
}

#[test]
fn a_broken_from_blocks_and_keeps_what_it_wrote() {
    let fx = Fx::new();
    // Detected through its user file; the entry has the default clients.
    fx.write(&fx.claude(), "{}\n");
    fx.put_server(stdio("api", &[], &["serve"]));
    let root = fx.th.projects().join("broken");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"broken-a1b2\"\n\n[servers.api]\nfrom = \"api\"\n",
    );
    fx.know_projects(&[("broken-a1b2", &root)]);
    fx.trust(&root);
    fx.apply();
    let mcp = root.join(".mcp.json");
    let written = fx.read(&mcp);
    assert!(json_entry(&written, "api").is_some());

    // The template is deleted from the store: the entry is reported
    // blocked, and what it wrote stays.
    fx.remove_server("api");
    let plan = fx.plan();
    let api = entry(&plan, &mcp, "api");
    assert_eq!(api.state, EntryState::Blocked, "{api:#?}");
    assert_eq!(
        api.problem.as_ref().unwrap().code(),
        ErrorCode::UnknownTemplate
    );
    fx.apply();
    assert_eq!(fx.read(&mcp), written);
    assert_eq!(fx.ledger().entries.len(), 1);
}

#[test]
fn git_errors_block_private_entries() {
    let fx = Fx::new();
    let root = fx.project_from("tracked", "broken-git");
    git_init(&root);
    // A repository git cannot open: whether files are tracked is unknown.
    std::fs::write(root.join(".git").join("config"), "[[[ not a config\n").unwrap();
    fx.know_projects(&[("tracked-e5f6", &root)]);
    fx.trust(&root);
    let omp_file = root.join(".omp").join("mcp.json");
    let plan = fx.plan();
    let notes = entry(&plan, &omp_file, "notes");
    assert_eq!(notes.state, EntryState::Blocked, "{notes:#?}");
    assert_eq!(notes.problem.as_ref().unwrap().code(), ErrorCode::GitError);
    fx.apply();
    assert!(!omp_file.exists());
}

#[test]
fn a_local_override_leaves_the_shared_file_as_committed() {
    let fx = Fx::new();
    let root = fx.th.projects().join("override");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"override-a1b2\"\n\n[defaults]\nclients = [\"claude-code\", \"omp\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nargs = [\"--shared\"]\n",
    );
    fx.know_projects(&[("override-a1b2", &root)]);
    fx.trust(&root);
    fx.apply();
    let mcp = root.join(".mcp.json");
    let shared = fx.read(&mcp);

    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\n\n[servers.docs]\nargs = [\"--local\"]\n",
    );
    fx.trust(&root);
    fx.apply();
    // Claude Code: the shared file is unchanged; the merged entry is
    // private, where it takes precedence.
    assert_eq!(fx.read(&mcp), shared);
    assert_eq!(
        claude_project(&fx, &root)["mcpServers"]["docs"]["args"],
        serde_json::json!(["--local"])
    );
    // omp has no private target: its project file holds the merged entry.
    let omp = json_entry(&fx.read(&root.join(".omp").join("mcp.json")), "docs").unwrap();
    assert_eq!(omp["args"], serde_json::json!(["--local"]));
}

#[test]
fn missing_and_failed_projects_keep_their_entries() {
    let fx = Fx::new();
    let root = private_project(&fx, "kept", "kept-a1b2");
    fx.apply();
    let owned = fx.ledger();
    assert_eq!(owned.entries.len(), 1);
    let claude = fx.read(&fx.claude());
    let kept = |fx: &Fx| {
        assert_eq!(fx.read(&fx.claude()), claude);
        assert_eq!(fx.ledger(), owned);
    };

    // Missing: the folder moved away.
    let moved = fx.th.projects().join("moved");
    std::fs::rename(&root, &moved).unwrap();
    fx.apply();
    kept(&fx);
    std::fs::rename(&moved, &root).unwrap();

    // Failed: a manifest that does not parse, one of another project, none.
    for text in ["schema = [", "schema = 1\nid = \"other-a1b2\"\n"] {
        fx.manifest(&root, LOCAL_FILE, text);
        let plan = fx.plan();
        assert!(
            plan.targets.iter().all(|t| t.project_id.is_none()),
            "{plan:#?}"
        );
        assert!(!plan.warnings.is_empty());
        fx.apply();
        kept(&fx);
    }
    std::fs::remove_file(root.join(".loadout").join(LOCAL_FILE)).unwrap();
    let plan = fx.plan();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code() == ErrorCode::NotFound)
    );
    fx.apply();
    kept(&fx);
}

#[test]
fn a_project_at_the_home_directory_is_never_planned() {
    let fx = Fx::new();
    let home = fx.th.home();
    fx.manifest(
        &home,
        LOCAL_FILE,
        "schema = 1\nid = \"home-a1b2\"\n\n[servers.notes]\ntransport = \"stdio\"\n\
         command = \"notes-mcp\"\nclients = [\"claude-code\"]\n",
    );
    fx.know_projects(&[("home-a1b2", &home)]);
    fx.trust(&home);
    let plan = fx.plan();
    assert!(
        plan.targets.iter().all(|t| t.project_id.is_none()),
        "{plan:#?}"
    );
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code() == ErrorCode::InvalidPath)
    );
}

#[test]
fn a_differently_spelled_root_is_the_same_project() {
    let fx = Fx::new();
    let root = fx.th.projects().join("spelling");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"spelling-a1b2\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    );
    // Another case and separator for the folder itself (the test root keeps
    // its spelling: the rooted test file system checks it exactly).
    let other = if cfg!(windows) {
        format!("{}/SPELLING/", fx.th.projects().to_string_lossy())
    } else {
        format!("{}/", root.to_string_lossy())
    };
    let other = Path::new(&other);
    fx.know_projects(&[("spelling-a1b2", other)]);
    // Trusted under the original spelling, selected under it too.
    fx.trust(&root);
    let only = fx.plan_with(&PlanOptions {
        projects: ProjectSelection::Only(root.clone()),
        ..PlanOptions::default()
    });
    let project: Vec<&TargetPlan> = only
        .targets
        .iter()
        .filter(|t| t.project_id.as_deref() == Some("spelling-a1b2"))
        .collect();
    assert_eq!(project.len(), 1, "{only:#?}");
    assert!(project[0].problem.is_none(), "{:#?}", project[0]);
    assert!(
        !only
            .warnings
            .iter()
            .any(|w| w.code() == ErrorCode::NotFound)
    );
}

#[test]
fn an_unreadable_trust_file_blocks_projects_only() {
    let fx = Fx::new();
    fx.put_server(stdio("fetch", &["claude-code"], &["-y", "fetch-mcp"]));
    private_project(&fx, "distrust", "distrust-a1b2");
    fx.write(&fx.home.state_dir().join("trust.json"), "{ not json");
    let plan = fx.plan();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code() == ErrorCode::ParseError)
    );
    let project = plan
        .targets
        .iter()
        .find(|t| t.project_id.is_some())
        .unwrap();
    assert_eq!(
        project.problem.as_ref().unwrap().code(),
        ErrorCode::TrustRequired
    );
    assert_eq!(
        entry(&plan, &fx.claude(), "fetch").state,
        EntryState::PendingCreate
    );
}

#[test]
fn the_project_folder_in_a_shared_file_warns() {
    let fx = Fx::new();
    let root = fx.th.projects().join("folder");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"folder-a1b2\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nargs = [\"${project}\"]\n",
    );
    fx.know_projects(&[("folder-a1b2", &root)]);
    fx.trust(&root);
    let plan = fx.plan();
    let mcp = root.join(".mcp.json");
    assert!(
        target(&plan, &mcp)
            .warnings
            .iter()
            .any(|w| w.code() == ErrorCode::PersonalPathInSharedFile),
        "{plan:#?}"
    );
    let docs = entry(&plan, &mcp, "docs");
    assert_eq!(docs.state, EntryState::PendingCreate);
    assert_eq!(
        docs.desired
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .get("args"),
        Some(&agent_loadout_core::formats::NativeValue::Array(vec![
            agent_loadout_core::formats::NativeValue::String(root.to_string_lossy().into_owned())
        ]))
    );
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_client_without_a_mechanism_cannot_suppress() {
    let fx = Fx::new();
    fx.put_server(stdio("slack", &["claude-desktop"], &["-y", "slack-mcp"]));
    let root = fx.th.projects().join("desktop");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"desktop-a1b2\"\n\n[servers.slack]\nenabled = false\n\
         clients = [\"claude-desktop\"]\n",
    );
    fx.know_projects(&[("desktop-a1b2", &root)]);
    fx.trust(&root);
    let plan = fx.plan();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code() == ErrorCode::CannotSuppressInProject),
        "{plan:#?}"
    );
    assert!(plan.targets.iter().all(|t| t.project_id.is_none()));
}

#[test]
fn suppressions_of_a_server_that_does_not_load_stay() {
    let fx = Fx::new();
    fx.put_server(stdio(
        "slack",
        &["claude-code", "omp"],
        &["-y", "slack-mcp"],
    ));
    let root = fx.th.projects().join("quiet");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"quiet-c3d4\"\n\n[servers.slack]\nenabled = false\n\
         clients = [\"claude-code\", \"omp\"]\n",
    );
    fx.know_projects(&[("quiet-c3d4", &root)]);
    fx.trust(&root);
    fx.apply();
    let omp_file = root.join(".omp").join("mcp.json");
    assert_eq!(
        json_entry(&fx.read(&omp_file), "slack").unwrap()["enabled"],
        false
    );
    assert_eq!(
        claude_project(&fx, &root)["disabledMcpServers"],
        serde_json::json!(["slack"])
    );

    // A broken hand edit of the server file (working tree only): whether
    // the server is still enabled is unknown, so its suppressions stay.
    let path = fx.home.store_dir().join("servers").join("slack.toml");
    std::fs::write(&path, "schema = 1\nid = \"slack\"\ntransport = \n").unwrap();
    let plan = fx.plan();
    for target in plan.targets.iter().filter(|t| t.project_id.is_some()) {
        assert!(target.after.is_none(), "{target:#?}");
    }
    let omp = entry(&plan, &omp_file, "slack");
    assert_eq!(omp.state, EntryState::Blocked, "{omp:#?}");
    assert_eq!(omp.problem.as_ref().unwrap().code(), ErrorCode::ParseError);
    let member = plan
        .targets
        .iter()
        .filter(|t| t.kind == EntryKind::ListMember)
        .flat_map(|t| &t.entries)
        .find(|e| e.name == "slack")
        .unwrap_or_else(|| panic!("{plan:#?}"));
    assert_eq!(member.state, EntryState::Blocked, "{member:#?}");
    assert_eq!(member.action, agent_loadout_core::apply::Action::None);
}

/// Gate V2 FB-10 (PI-8): a project install of `pi-mcp-adapter` (`pi install
/// -l`) is reported on that project's Pi target and on every Pi row of the
/// project's effective view; the user file reports user installs only.
#[test]
fn a_project_install_of_pi_mcp_adapter_warns() {
    use agent_loadout_core::CoreError;
    use agent_loadout_core::layering::effective;

    const INSTALLED: &str = r#"{"packages": ["npm:pi-mcp-adapter"]}"#;
    let replaced = |warnings: &[CoreError]| {
        warnings
            .iter()
            .any(|w| w.code() == ErrorCode::ClientMcpReplacedByExtension)
    };
    let fx = Fx::new();
    fx.put_server(stdio("github", &["pi"], &["-y", "github-mcp"]));
    let root = fx.th.projects().join("adapted");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"adapted-a1b2\"\n\n[defaults]\nclients = [\"pi\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    );
    fx.know_projects(&[("adapted-a1b2", &root)]);
    fx.trust(&root);
    fx.write(&root.join(".pi").join("settings.json"), INSTALLED);

    let project_file = root.join(".pi").join("mcp.json");
    let agent_dir = fx.th.home().join(".pi").join("agent");
    let user_file = agent_dir.join("mcp.json");
    let plan = fx.plan();
    assert!(
        replaced(&target(&plan, &project_file).warnings),
        "{plan:#?}"
    );
    assert!(!replaced(&target(&plan, &user_file).warnings), "{plan:#?}");

    let store = fx.store();
    let pi = ["pi".to_owned()];
    let view = effective(Some(&store), &fx.ax(), &root, Some(&pi)).unwrap();
    let rows = &view.clients["pi"];
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["docs", "github"]);
    assert!(rows.iter().all(|row| replaced(&row.warnings)), "{rows:#?}");
    let elsewhere = fx.th.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let view = effective(Some(&store), &fx.ax(), &elsewhere, Some(&pi)).unwrap();
    assert!(
        view.clients["pi"]
            .iter()
            .all(|row| !replaced(&row.warnings))
    );

    // A user install shows on the user file and outside the project too.
    fx.write(&agent_dir.join("settings.json"), INSTALLED);
    let plan = fx.plan();
    assert!(replaced(&target(&plan, &user_file).warnings), "{plan:#?}");
    let view = effective(Some(&store), &fx.ax(), &elsewhere, Some(&pi)).unwrap();
    let rows = &view.clients["pi"];
    assert!(!rows.is_empty() && rows.iter().all(|row| replaced(&row.warnings)));
}

/// Gate V2 FB-13 (CUR-2): Cursor lists new project servers as Disabled
/// until each is enabled in Customize > MCPs. A Cursor project target with
/// entries Loadout writes or owns, the apply report and Cursor's project
/// rows of the effective view carry `client_enable_required`; the user file
/// and user rows do not.
#[test]
fn cursor_project_entries_need_a_one_time_enable() {
    use agent_loadout_core::CoreError;
    use agent_loadout_core::layering::{Layer, effective};

    let hint = |warnings: &[CoreError]| {
        warnings
            .iter()
            .find(|w| w.code() == ErrorCode::ClientEnableRequired)
            .map(|w| w.message().to_owned())
    };
    let fx = Fx::new();
    fx.put_server(stdio("github", &["cursor"], &["-y", "github-mcp"]));
    let root = fx.th.projects().join("cursed");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"cursed-a1b2\"\n\n[defaults]\nclients = [\"cursor\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    );
    fx.know_projects(&[("cursed-a1b2", &root)]);
    fx.trust(&root);

    let project_file = root.join(".cursor").join("mcp.json");
    let user_file = fx.th.home().join(".cursor").join("mcp.json");
    let plan = fx.plan();
    let message =
        hint(&target(&plan, &project_file).warnings).unwrap_or_else(|| panic!("{plan:#?}"));
    assert!(message.contains("Customize > MCPs"), "{message}");
    assert!(message.contains("\"docs\""), "{message}");
    assert_eq!(hint(&target(&plan, &user_file).warnings), None);

    let (report, _) = fx.apply();
    let written = report
        .targets
        .iter()
        .find(|t| t.file == project_file)
        .unwrap_or_else(|| panic!("{report:#?}"));
    assert!(
        written.written && hint(&written.warnings).is_some(),
        "{written:#?}"
    );

    // Owned and synced: still a hint, since Loadout cannot see the toggle.
    let plan = fx.plan();
    assert!(hint(&target(&plan, &project_file).warnings).is_some());

    let store = fx.store();
    let cursor = ["cursor".to_owned()];
    let view = effective(Some(&store), &fx.ax(), &root, Some(&cursor)).unwrap();
    for row in &view.clients["cursor"] {
        let project = row.winner.layer == Layer::Project;
        assert_eq!(hint(&row.warnings).is_some(), project, "{row:#?}");
    }
    assert!(
        view.clients["cursor"]
            .iter()
            .any(|row| row.name == "docs" && row.loaded && row.winner.layer == Layer::Project)
    );
}
