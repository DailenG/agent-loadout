//! Discover projects (#3): folders Claude Code (`~/.claude.json`
//! `projects`), Codex (trusted `[projects."<folder>"]`), omp and Pi (the
//! session header's `cwd`) know, minus folders that are gone, the home
//! folder and the projects this machine knows; adding them is one commit
//! and trusts none. Every client file is written into a `TempHome`.

mod support;

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::layering::discover::{self, candidate_id};
use agent_loadout_core::layering::{Project, TrustState, TrustStore};
use support::Fx;

fn folder(fx: &Fx, name: &str) -> PathBuf {
    let root = fx.th.projects().join(name);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A session file of omp or Pi: omp writes a title line first.
fn session(fx: &Fx, agent: &Path, dir: &str, file: &str, cwd: &Path, title_first: bool) {
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": "01J0000000000000000000000",
        "timestamp": "2026-11-15T02:01:16.155Z",
        "cwd": text(cwd),
    });
    let mut body = String::new();
    if title_first {
        body.push_str("{\"type\":\"title\",\"v\":1,\"title\":\"x\",\"pad\":\"   \"}\n");
    }
    body.push_str(&format!("{header}\n"));
    body.push_str("{\"type\":\"model_change\",\"id\":\"a\"}\n");
    fx.write(&agent.join("sessions").join(dir).join(file), &body);
}

/// Client files naming `a` (Claude Code and omp), `b` (Codex, trusted, with
/// a manifest), `c` (Codex, untrusted: left out), `d` (Pi), `known` (a
/// project this machine knows), a missing folder and the home folder.
fn clients(fx: &Fx) -> [PathBuf; 4] {
    let a = folder(fx, "a");
    let b = folder(fx, "b");
    let c = folder(fx, "c");
    let d = folder(fx, "d");
    let known = folder(fx, "known");
    fx.manifest(
        &known,
        "project.local.toml",
        "schema = 1\nid = \"known-0001\"\n",
    );
    fx.know_projects(&[("known-0001", &known)]);
    fx.manifest(
        &b,
        "project.toml",
        "schema = 1\nid = \"b-0002\"\nname = \"Bee\"\n",
    );

    let missing = fx.th.projects().join("gone");
    let mut projects = serde_json::Map::new();
    for root in [&a, &known, &missing, &fx.th.home()] {
        projects.insert(text(root), serde_json::json!({ "allowedTools": [] }));
    }
    let claude = serde_json::json!({ "numStartups": 3, "projects": projects });
    fx.write(&fx.claude(), &claude.to_string());

    let codex = format!(
        "model = \"o3\"\n\n[projects.'{}']\ntrust_level = \"trusted\"\n\n[projects.'{}']\n\
         trust_level = \"untrusted\"\n",
        text(&b),
        text(&c)
    );
    fx.write(&fx.th.home().join(".codex").join("config.toml"), &codex);

    let omp = fx.th.home().join(".omp").join("agent");
    session(
        fx,
        &omp,
        "--a--",
        "2026-11-14T00-00-00-000Z_1.jsonl",
        &c,
        true,
    );
    session(
        fx,
        &omp,
        "--a--",
        "2026-11-15T00-00-00-000Z_2.jsonl",
        &a,
        true,
    );
    let pi = fx.th.home().join(".pi").join("agent");
    session(
        fx,
        &pi,
        "--d--",
        "2026-11-15T00-00-00-000Z_3.jsonl",
        &d,
        false,
    );
    [a, b, c, d]
}

#[test]
fn folders_the_clients_know_are_offered_once_each() {
    let fx = Fx::new();
    let [a, b, _c, d] = clients(&fx);
    let store = fx.store();

    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    let roots: Vec<&Path> = found
        .candidates
        .iter()
        .map(|candidate| candidate.root.as_path())
        .collect();
    assert_eq!(roots, [a.as_path(), b.as_path(), d.as_path()]);
    let clients: Vec<&[&str]> = found
        .candidates
        .iter()
        .map(|candidate| candidate.clients.as_slice())
        .collect();
    assert_eq!(
        clients,
        [&["claude-code", "omp"][..], &["codex"][..], &["pi"][..]]
    );
    let first = &found.candidates[0];
    assert_eq!(first.id, candidate_id(&a));
    assert!(first.id.starts_with("d-") && first.id.len() == 14);
    assert_eq!(first.name, "a");
    assert!(first.manifest.is_none());
    let bee = found.candidates[1].manifest.as_ref().unwrap();
    assert_eq!(bee.id, "b-0002");

    // A client file that does not parse is a warning; the others count.
    fx.write(&fx.claude(), "{ not json");
    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    assert_eq!(found.warnings.len(), 1);
    assert_eq!(found.warnings[0].code(), ErrorCode::ParseError);
    assert_eq!(found.candidates.len(), 3, "omp still names a");
}

#[test]
fn adding_is_one_commit_and_trusts_nothing() {
    let fx = Fx::new();
    let [a, b, _c, d] = clients(&fx);
    let store = fx.store();
    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    let ids: Vec<String> = found
        .candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect();

    let before = fx.commit_count();
    let done = discover::add(&store, &fx.env, &fx.fs, &fx.home, &ids).unwrap();
    assert!(done.failed.is_empty(), "{:?}", done.failed);
    assert_eq!(fx.commit_count(), before + 1, "one commit for every folder");
    assert_eq!(fx.log()[0].summary, "project: add 3 discovered projects");
    assert_eq!(done.added.len(), 3);
    let created: Vec<bool> = done
        .added
        .iter()
        .map(|added| added.created_manifest)
        .collect();
    assert_eq!(created, [true, false, true]);
    // A folder without a manifest got a private one.
    let local = a.join(".loadout").join("project.local.toml");
    assert!(local.is_file());
    assert!(!b.join(".loadout").join("project.local.toml").exists());

    let trust = TrustStore::load(&fx.fs, &fx.home.state_dir()).unwrap();
    for root in [&a, &b, &d] {
        let project = Project::load(&fx.fs, root).unwrap().unwrap();
        assert_eq!(
            trust.state(&project),
            TrustState::Untrusted,
            "{} is not trusted after add",
            root.display()
        );
    }
    // No mirror: the store holds the content last trusted, and there is none.
    assert!(
        store
            .read_to_string(&format!("projects/{}/project.toml", "b-0002"))
            .unwrap()
            .is_none()
    );

    // Known now: nothing left to discover, and a stale id is refused.
    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    assert!(found.candidates.is_empty());
    let err = discover::add(&store, &fx.env, &fx.fs, &fx.home, &ids).unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);
}

#[test]
fn a_folder_that_cannot_be_bound_is_reported_and_the_rest_added() {
    let fx = Fx::new();
    let [a, b, _c, _d] = clients(&fx);
    // `b`'s project id is known at another folder that still holds it.
    let twin = folder(&fx, "twin");
    fx.manifest(&twin, "project.toml", "schema = 1\nid = \"b-0002\"\n");
    let known = fx.th.projects().join("known");
    fx.know_projects(&[("known-0001", &known), ("b-0002", &twin)]);
    let store = fx.store();

    let ids = vec![candidate_id(&a), candidate_id(&b)];
    let done = discover::add(&store, &fx.env, &fx.fs, &fx.home, &ids).unwrap();
    assert_eq!(done.added.len(), 1);
    assert_eq!(done.added[0].root, a);
    assert_eq!(done.failed.len(), 1);
    assert_eq!(done.failed[0].root, b);
    assert_eq!(done.failed[0].error.code(), ErrorCode::AlreadyExists);
    assert_eq!(
        fx.log()[0].summary,
        format!("project: add {}", done.added[0].project_id)
    );
}

/// Claude Code writes `projects` keys with `/` on Windows
/// (`C:/Users/<user>/code/app`): the same folder as Codex's `C:\...`
/// spelling, and known once added.
#[test]
fn forward_slash_keys_are_the_same_folder() {
    let fx = Fx::new();
    let a = folder(&fx, "a");
    let forward = text(&a).replace('\\', "/");
    let claude = serde_json::json!({ "projects": { forward.clone(): {} } });
    fx.write(&fx.claude(), &claude.to_string());
    fx.write(
        &fx.th.home().join(".codex").join("config.toml"),
        &format!("[projects.'{}']\ntrust_level = \"trusted\"\n", text(&a)),
    );
    let store = fx.store();

    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    assert_eq!(found.candidates.len(), 1, "{:?}", found.candidates);
    let candidate = &found.candidates[0];
    assert_eq!(candidate.clients, ["claude-code", "codex"]);
    assert_eq!(candidate.id, candidate_id(&a));
    assert_eq!(candidate.id, candidate_id(Path::new(&forward)));

    let ids = std::slice::from_ref(&candidate.id);
    let done = discover::add(&store, &fx.env, &fx.fs, &fx.home, ids).unwrap();
    assert_eq!(done.added.len(), 1);
    let found = discover::discover(Some(&store), &fx.env, &fx.fs, &fx.home).unwrap();
    assert!(found.candidates.is_empty(), "{:?}", found.candidates);
}
