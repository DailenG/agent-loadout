//! `loadout project init|add|ls|rm|trust|share` and project selection on
//! `apply`/`status` (SPEC 5.5, 6, 14, 17), against a temp `LOADOUT_HOME`,
//! a temp profile and git2-created project repositories under
//! `TempHome::projects()`.

use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_testkit::TempHome;
use serde_json::Value;

struct Cli {
    th: TempHome,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self { th, home }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.th
            .command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    /// Runs with `--json` and returns the exit code and the document.
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let output = self.run(&[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }

    /// A new project folder (a git repository when `git`).
    fn folder(&self, name: &str, git: bool) -> (PathBuf, Option<git2::Repository>) {
        let root = self.th.projects().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let repo = git.then(|| {
            let repo = git2::Repository::init(&root).unwrap();
            std::fs::write(root.join("README.md"), "# app\n").unwrap();
            commit(&repo, &["README.md"]);
            repo
        });
        (root, repo)
    }

    /// `project init` and the new project id.
    fn init(&self, root: &Path, extra: &[&str]) -> String {
        let root = root.to_str().unwrap();
        let doc = self.ok(&[&["project", "init", root][..], extra].concat());
        doc["project"]["id"].as_str().unwrap().to_owned()
    }

    fn claude(&self) -> Value {
        let text = std::fs::read_to_string(self.th.home().join(".claude.json"))
            .unwrap_or_else(|_| "{}".to_owned());
        serde_json::from_str(&text).unwrap()
    }

    /// Claude Code's private entry `name` for any project folder.
    fn claude_private(&self, name: &str) -> Option<Value> {
        let claude = self.claude();
        claude["projects"]
            .as_object()?
            .values()
            .find_map(|project| project["mcpServers"].get(name).cloned())
    }

    fn mirror(&self, id: &str, file: &str) -> Option<String> {
        std::fs::read_to_string(self.home.join("store").join("projects").join(id).join(file)).ok()
    }

    fn projects(&self) -> Vec<Value> {
        self.ok(&["project", "ls"])["projects"]
            .as_array()
            .unwrap()
            .clone()
    }
}

fn manifest(root: &Path, file: &str) -> PathBuf {
    root.join(".loadout").join(file)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn append(path: &Path, text: &str) {
    let mut current = read(path);
    current.push_str(text);
    std::fs::write(path, current).unwrap();
}

/// Stages `files` (`/`-separated, relative to the work tree) and commits.
fn commit(repo: &git2::Repository, files: &[&str]) {
    let mut index = repo.index().unwrap();
    for file in files {
        index.add_path(Path::new(file)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature =
        git2::Signature::new("test", "test@example.invalid", &git2::Time::new(0, 0)).unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "fixture",
        &tree,
        &parents,
    )
    .unwrap();
}

/// Paths git reports as changed or untracked (ignored ones excluded).
fn git_status(repo: &git2::Repository) -> Vec<String> {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    repo.statuses(Some(&mut options))
        .unwrap()
        .iter()
        .map(|entry| entry.path().unwrap_or_default().to_owned())
        .collect()
}

fn exclude(repo: &git2::Repository) -> String {
    std::fs::read_to_string(repo.path().join("info").join("exclude")).unwrap_or_default()
}

/// Every target of an apply report or plan whose `project_id` is `id`.
fn project_targets<'a>(targets: &'a Value, id: &str) -> Vec<&'a Value> {
    targets
        .as_array()
        .unwrap()
        .iter()
        .filter(|target| target["project_id"] == id)
        .collect()
}

#[test]
fn init_defaults_to_private_and_hides_the_manifest_from_git() {
    let cli = Cli::new();
    let (root, repo) = cli.folder("My App", true);
    let repo = repo.unwrap();
    let (code, doc) = cli.json(&["project", "init", root.to_str().unwrap()]);
    assert_eq!(code, 0, "{doc:#}");
    let id = doc["project"]["id"].as_str().unwrap().to_owned();
    let (slug, suffix) = id.rsplit_once('-').unwrap();
    assert_eq!(slug, "my-app");
    assert_eq!(suffix.len(), 4);
    assert!(
        suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    );
    assert_eq!(doc["project"]["layout"], "private");
    assert!(doc["commit"].is_string(), "{doc:#}");

    let local = read(&manifest(&root, "project.local.toml"));
    assert!(local.contains(&format!("id = \"{id}\"")), "{local}");
    assert!(local.contains("name = \"My App\""), "{local}");
    assert!(!manifest(&root, "project.toml").exists());
    assert!(!manifest(&root, ".gitignore").exists());
    assert!(
        exclude(&repo).lines().any(|line| line == "/.loadout/"),
        "{}",
        exclude(&repo)
    );
    assert_eq!(git_status(&repo), Vec::<String>::new());

    // Overlay, mirror and trust were recorded in that one commit.
    assert_eq!(cli.mirror(&id, "project.local.toml"), Some(local));
    let projects = cli.projects();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["id"], id.as_str());
    assert_eq!(projects[0]["path"], root.to_str().unwrap());
    assert_eq!(projects[0]["layout"], "private");
    assert_eq!(projects[0]["trust"], "trusted");
    let history = cli.ok(&["history"])["commits"].as_array().unwrap().clone();
    assert_eq!(history[0]["summary"], format!("project: init {id}"));

    // A second init in the same folder, or one in the home folder, fails.
    let (code, doc) = cli.json(&["project", "init", root.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "already_exists");
    let (code, doc) = cli.json(&["project", "init", cli.th.home().to_str().unwrap()]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_path");
    assert!(!manifest(&cli.th.home(), "project.local.toml").exists());
}

#[test]
fn init_shared_writes_project_toml_and_gitignore() {
    let cli = Cli::new();
    let (root, repo) = cli.folder("team", true);
    let repo = repo.unwrap();
    let id = cli.init(&root, &["--shared", "--name", "Team Tool"]);
    let shared = read(&manifest(&root, "project.toml"));
    assert!(shared.contains(&format!("id = \"{id}\"")), "{shared}");
    assert!(shared.contains("name = \"Team Tool\""), "{shared}");
    assert!(!manifest(&root, "project.local.toml").exists());
    assert_eq!(
        read(&manifest(&root, ".gitignore")).trim(),
        "project.local.toml"
    );
    assert!(!exclude(&repo).contains(".loadout"), "{}", exclude(&repo));
    // The shared manifest is for the project's repository.
    let status = git_status(&repo);
    assert!(
        status.contains(&".loadout/project.toml".to_owned()),
        "{status:?}"
    );
    assert_eq!(cli.mirror(&id, "project.toml"), Some(shared));
    assert_eq!(cli.projects()[0]["layout"], "shared");
}

#[test]
fn an_edited_manifest_blocks_apply_until_trusted() {
    let cli = Cli::new();
    let (root, _) = cli.folder("app", false);
    let id = cli.init(&root, &[]);
    let local = manifest(&root, "project.local.toml");
    append(
        &local,
        "\n[servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nclients = [\"claude-code\"]\n",
    );
    assert_eq!(cli.projects()[0]["trust"], "changed");

    let (code, report) = cli.json(&["apply"]);
    assert_eq!(code, 7, "trust_required exits 7: {report:#}");
    let blocked = project_targets(&report["targets"], &id);
    assert!(!blocked.is_empty(), "{report:#}");
    for target in &blocked {
        assert_eq!(target["error"]["code"], "trust_required", "{report:#}");
        assert_eq!(target["written"], false);
    }
    assert_eq!(cli.claude_private("docs"), None);

    // `status PATH` shows only that project, blocked.
    let status = cli.ok(&["status", root.to_str().unwrap()]);
    let targets = status["targets"].as_array().unwrap();
    assert!(!targets.is_empty());
    for target in targets {
        assert_eq!(target["project_id"], id.as_str());
        assert_eq!(target["problem"]["code"], "trust_required");
    }
    let system = cli.ok(&["status", "--scope", "system"]);
    assert!(
        system["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|target| target["project_id"].is_null()),
        "{system:#}"
    );

    // Trust shows what changed since the last trust, then applies.
    let trusted = cli.ok(&["project", "trust", root.join("sub").to_str().unwrap()]);
    let files = trusted["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{trusted:#}");
    assert_eq!(files[0]["file"], "project.local.toml");
    assert_eq!(files[0]["change"], "changed");
    assert!(
        files[0]["diff"]
            .as_str()
            .unwrap()
            .contains("+[servers.docs]"),
        "{trusted:#}"
    );
    assert_eq!(trusted["apply"]["outcome"], "ok", "{trusted:#}");
    assert_eq!(cli.claude_private("docs").unwrap()["command"], "docs-mcp");
    assert_eq!(cli.mirror(&id, "project.local.toml"), Some(read(&local)));
    assert_eq!(cli.projects()[0]["trust"], "trusted");

    // `apply PATH` covers only that project, every target with its id.
    let report = cli.ok(&["apply", root.to_str().unwrap()]);
    for target in report["targets"].as_array().unwrap() {
        assert_eq!(target["project_id"], id.as_str());
    }
}

#[test]
fn a_private_entry_never_lands_in_a_tracked_project_file() {
    let cli = Cli::new();
    let (root, repo) = cli.folder("tracked", true);
    let repo = repo.unwrap();
    let mcp = root.join(".mcp.json");
    let omp = root.join(".omp").join("mcp.json");
    std::fs::create_dir_all(omp.parent().unwrap()).unwrap();
    std::fs::write(&mcp, "{\n  \"mcpServers\": {}\n}\n").unwrap();
    std::fs::write(&omp, "{\n  \"mcpServers\": {}\n}\n").unwrap();
    commit(&repo, &[".mcp.json", ".omp/mcp.json"]);
    let id = cli.init(&root, &[]);
    append(
        &manifest(&root, "project.local.toml"),
        "\n[servers.notes]\ntransport = \"stdio\"\ncommand = \"notes-mcp\"\n\
         clients = [\"claude-code\", \"omp\"]\n",
    );
    let trusted = cli.ok(&["project", "trust", root.to_str().unwrap()]);

    // Claude Code: its private target, never the tracked `.mcp.json`.
    assert_eq!(cli.claude_private("notes").unwrap()["command"], "notes-mcp");
    assert_eq!(read(&mcp), "{\n  \"mcpServers\": {}\n}\n");
    // omp has no private target and its project file is tracked: blocked.
    assert_eq!(read(&omp), "{\n  \"mcpServers\": {}\n}\n");
    let targets = project_targets(&trusted["apply"]["targets"], &id);
    let omp_target = targets
        .iter()
        .find(|target| target["file"] == omp.to_str().unwrap())
        .unwrap_or_else(|| panic!("{trusted:#}"));
    assert_eq!(
        omp_target["entries"][0]["problem"]["code"], "private_in_tracked_file",
        "{trusted:#}"
    );
    assert_eq!(git_status(&repo), Vec::<String>::new());
}

#[test]
fn project_rm_removes_owned_project_entries() {
    let cli = Cli::new();
    let (root, _) = cli.folder("gone", false);
    let id = cli.init(&root, &[]);
    append(
        &manifest(&root, "project.local.toml"),
        "\n[servers.notes]\ntransport = \"stdio\"\ncommand = \"notes-mcp\"\nclients = [\"claude-code\"]\n",
    );
    cli.ok(&["project", "trust", root.to_str().unwrap()]);
    assert!(cli.claude_private("notes").is_some());

    let removed = cli.ok(&["project", "rm", &id]);
    assert_eq!(removed["apply"]["outcome"], "ok", "{removed:#}");
    let targets = project_targets(&removed["apply"]["targets"], &id);
    assert_eq!(targets.len(), 1, "{removed:#}");
    assert_eq!(targets[0]["remove"], 1);
    assert_eq!(cli.claude_private("notes"), None);
    assert!(cli.projects().is_empty());
    // The folder and the store mirror stay.
    assert!(manifest(&root, "project.local.toml").exists());
    assert!(cli.mirror(&id, "project.local.toml").is_some());

    let (code, doc) = cli.json(&["project", "rm", &id]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "not_found");
}

#[test]
fn add_registers_without_trust() {
    let cli = Cli::new();
    let (root, _) = cli.folder("cloned", false);
    std::fs::create_dir_all(root.join(".loadout")).unwrap();
    std::fs::write(
        manifest(&root, "project.toml"),
        "schema = 1\nid = \"cloned-ab12\"\n\n[servers.docs]\ntransport = \"stdio\"\n\
         command = \"docs-mcp\"\nclients = [\"claude-code\"]\n",
    )
    .unwrap();
    let added = cli.ok(&["project", "add", root.to_str().unwrap()]);
    assert_eq!(added["project"]["id"], "cloned-ab12");
    assert_eq!(added["trust"], "untrusted");
    assert_eq!(cli.mirror("cloned-ab12", "project.toml"), None);
    let projects = cli.projects();
    assert_eq!(projects[0]["trust"], "untrusted");
    assert_eq!(projects[0]["layout"], "shared");

    let trusted = cli.ok(&["project", "trust", root.to_str().unwrap()]);
    assert_eq!(trusted["files"][0]["change"], "new");
    assert!(
        trusted["files"][0]["content"]
            .as_str()
            .unwrap()
            .contains("docs-mcp")
    );
    // Shared-only: the project's `.mcp.json`.
    let docs: Value = serde_json::from_str(&read(&root.join(".mcp.json"))).unwrap();
    assert_eq!(docs["mcpServers"]["docs"]["command"], "docs-mcp");
}

#[test]
fn trust_exits_on_its_own_project_not_on_another_untrusted_one() {
    let cli = Cli::new();
    let mut roots = Vec::new();
    for id in ["first-ab12", "second-cd34"] {
        let (root, _) = cli.folder(id, false);
        std::fs::create_dir_all(root.join(".loadout")).unwrap();
        std::fs::write(
            manifest(&root, "project.toml"),
            format!(
                "schema = 1\nid = \"{id}\"\n\n[servers.docs]\ntransport = \"stdio\"\n\
                 command = \"docs-mcp\"\nclients = [\"claude-code\"]\n"
            ),
        )
        .unwrap();
        cli.ok(&["project", "add", root.to_str().unwrap()]);
        roots.push(root);
    }

    // The instant apply after the trust still reports the other project as
    // blocked, but that is not this command's failure (#10).
    let trusted = cli.ok(&["project", "trust", roots[0].to_str().unwrap()]);
    let targets = &trusted["apply"]["targets"];
    assert!(
        project_targets(targets, "second-cd34")
            .iter()
            .any(|target| target["error"]["code"] == "trust_required"),
        "{trusted:#}"
    );
    let docs: Value = serde_json::from_str(&read(&roots[0].join(".mcp.json"))).unwrap();
    assert_eq!(docs["mcpServers"]["docs"]["command"], "docs-mcp");

    // A whole-machine apply still exits 7 for the untrusted project.
    let (code, _) = cli.json(&["apply"]);
    assert_eq!(code, 7);
}

#[test]
fn share_moves_private_servers_into_project_toml_with_comments() {
    let cli = Cli::new();
    let (root, repo) = cli.folder("shareable", true);
    let repo = repo.unwrap();
    let id = cli.init(&root, &[]);
    let local = manifest(&root, "project.local.toml");
    append(
        &local,
        "\n# Docs for the whole team.\n[servers.docs]\ntransport = \"stdio\"\n\
         command = \"docs-mcp\" # pinned\nclients = [\"claude-code\"]\n\n\
         [servers.docs.env]\nLEVEL = \"info\"\n\n\
         # Mine only.\n[servers.notes]\ntransport = \"stdio\"\ncommand = \"notes-mcp\"\n\
         clients = [\"claude-code\"]\n",
    );
    cli.ok(&["project", "trust", root.to_str().unwrap()]);

    let shared = cli.ok(&["project", "share", &id, "--server", "docs"]);
    assert_eq!(shared["servers"], serde_json::json!(["docs"]));
    assert_eq!(shared["created"], true);
    assert_eq!(shared["project"]["layout"], "shared+local");
    let project_toml = read(&manifest(&root, "project.toml"));
    assert!(
        project_toml.contains(&format!("id = \"{id}\"")),
        "{project_toml}"
    );
    assert!(
        project_toml.contains("# Docs for the whole team.\n[servers.docs]"),
        "{project_toml}"
    );
    assert!(project_toml.contains("# pinned"), "{project_toml}");
    assert!(
        project_toml.contains("[servers.docs.env]\nLEVEL = \"info\""),
        "{project_toml}"
    );
    let rest = read(&local);
    assert!(!rest.contains("docs"), "{rest}");
    assert!(rest.contains("# Mine only.\n[servers.notes]"), "{rest}");
    assert!(rest.contains(&format!("id = \"{id}\"")), "{rest}");
    assert_eq!(
        read(&manifest(&root, ".gitignore")).trim(),
        "project.local.toml"
    );
    // The shared file can now be committed; the private one stays hidden.
    assert!(!exclude(&repo).contains("/.loadout/"), "{}", exclude(&repo));
    let status = git_status(&repo);
    assert!(
        status.contains(&".loadout/project.toml".to_owned()),
        "{status:?}"
    );
    assert!(
        !status.contains(&".loadout/project.local.toml".to_owned()),
        "{status:?}"
    );
    assert_eq!(cli.mirror(&id, "project.toml"), Some(project_toml));
    assert_eq!(cli.mirror(&id, "project.local.toml"), Some(rest));
    assert_eq!(cli.projects()[0]["trust"], "trusted");
    // Shared now: the docs entry moved to `.mcp.json`.
    let mcp: Value = serde_json::from_str(&read(&root.join(".mcp.json"))).unwrap();
    assert!(mcp["mcpServers"]["docs"].is_object(), "{mcp:#}");
    assert_eq!(cli.claude_private("docs"), None);
    assert!(cli.claude_private("notes").is_some());

    // Sharing the rest leaves a local file without servers or id.
    cli.ok(&["project", "share", &id]);
    let rest = read(&local);
    assert_eq!(rest.trim(), "schema = 1", "{rest}");

    let (code, doc) = cli.json(&["project", "share", &id]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "not_found");
}

#[test]
fn a_second_checkout_of_a_known_project_is_refused_until_rm() {
    let cli = Cli::new();
    let (first, _) = cli.folder("first", false);
    let id = cli.init(&first, &["--shared"]);
    append(
        &manifest(&first, "project.toml"),
        "\n[servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nclients = [\"claude-code\"]\n",
    );
    cli.ok(&["project", "trust", first.to_str().unwrap()]);
    let first_mcp = read(&first.join(".mcp.json"));

    // A clone carries the same id.
    let (second, _) = cli.folder("second", false);
    std::fs::create_dir_all(second.join(".loadout")).unwrap();
    std::fs::copy(
        manifest(&first, "project.toml"),
        manifest(&second, "project.toml"),
    )
    .unwrap();
    for command in ["add", "trust"] {
        let (code, doc) = cli.json(&["project", command, second.to_str().unwrap()]);
        assert_eq!(code, 1, "{command}: {doc:#}");
        assert_eq!(doc["error"]["code"], "already_exists");
        let message = doc["error"]["message"].as_str().unwrap();
        assert!(message.contains(first.to_str().unwrap()), "{message}");
        assert!(message.contains(second.to_str().unwrap()), "{message}");
        assert!(message.contains(&format!("project rm {id}")), "{message}");
    }
    // Nothing moved: the first checkout keeps its project and its entries.
    let projects = cli.projects();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["path"], first.to_str().unwrap());
    assert_eq!(read(&first.join(".mcp.json")), first_mcp);
    assert!(!second.join(".mcp.json").exists());

    // A folder that is gone is a move: the project follows it.
    std::fs::remove_dir_all(&first).unwrap();
    cli.ok(&["project", "trust", second.to_str().unwrap()]);
    let projects = cli.projects();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["path"], second.to_str().unwrap());
    assert_eq!(projects[0]["trust"], "trusted");
}

#[test]
fn a_failed_trust_commit_leaves_the_trust_state_unchanged() {
    let cli = Cli::new();
    let (root, _) = cli.folder("leaky", false);
    let id = cli.init(&root, &[]);
    let local = manifest(&root, "project.local.toml");
    let trusted = read(&local);
    // Assembled at runtime so the repository holds no token-shaped string.
    let token = ["gh", "p_", "FAKEfake0123456789FAKEfake0123456789"].concat();
    append(
        &local,
        &format!(
            "\n[servers.leaky]\ntransport = \"stdio\"\ncommand = \"leaky-mcp\"\n\
             clients = [\"claude-code\"]\n\n[servers.leaky.env]\nUPSTREAM = \"{token}\"\n"
        ),
    );
    let trust_file = cli.home.join("state").join("trust.json");
    let before = read(&trust_file);

    let (code, doc) = cli.json(&["project", "trust", root.to_str().unwrap()]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["error"]["code"], "leak_detected");
    assert_eq!(read(&trust_file), before);
    assert_eq!(cli.projects()[0]["trust"], "changed");
    assert_eq!(cli.mirror(&id, "project.local.toml"), Some(trusted));
    // The instant apply never ran: no client file got the entry.
    assert_eq!(cli.claude_private("leaky"), None);

    // The next apply still blocks the project.
    let (code, report) = cli.json(&["apply"]);
    assert_eq!(code, 7, "trust_required exits 7: {report:#}");
    let targets = project_targets(&report["targets"], &id);
    assert!(!targets.is_empty(), "{report:#}");
    for target in targets {
        assert_eq!(target["error"]["code"], "trust_required", "{report:#}");
    }
}

#[test]
fn share_into_an_existing_project_toml_keeps_overrides_local() {
    let cli = Cli::new();
    let (root, repo) = cli.folder("both", true);
    let repo = repo.unwrap();
    let id = cli.init(&root, &["--shared"]);
    append(
        &manifest(&root, "project.toml"),
        "\n[servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nclients = [\"claude-code\"]\n",
    );
    let local = manifest(&root, "project.local.toml");
    std::fs::write(
        &local,
        "schema = 1\n\n# My docs flags.\n[servers.docs]\ntransport = \"stdio\"\n\
         command = \"docs-mcp\"\nargs = [\"--verbose\"]\nclients = [\"claude-code\"]\n\n\
         [servers.search]\ntransport = \"stdio\"\ncommand = \"search-mcp\"\n\
         clients = [\"claude-code\"]\n",
    )
    .unwrap();
    cli.ok(&["project", "trust", root.to_str().unwrap()]);
    let gitignore = read(&manifest(&root, ".gitignore"));
    let exclude_before = exclude(&repo);

    // An explicit override name collides; the default skips it.
    let (code, doc) = cli.json(&["project", "share", &id, "--server", "docs"]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["error"]["code"], "already_exists");
    let shared = cli.ok(&["project", "share", &id]);
    assert_eq!(shared["servers"], serde_json::json!(["search"]));
    assert_eq!(shared["created"], false);
    let rest = read(&local);
    assert!(rest.contains("# My docs flags.\n[servers.docs]"), "{rest}");
    assert!(!rest.contains("search"), "{rest}");
    let project_toml = read(&manifest(&root, "project.toml"));
    assert!(project_toml.contains("[servers.search]"), "{project_toml}");
    assert!(!project_toml.contains("--verbose"), "{project_toml}");
    // An existing shared layout keeps its ignore files as they were.
    assert_eq!(read(&manifest(&root, ".gitignore")), gitignore);
    assert_eq!(exclude(&repo), exclude_before);
    assert_eq!(cli.projects()[0]["trust"], "trusted");
    assert_eq!(cli.mirror(&id, "project.toml"), Some(project_toml));
}
