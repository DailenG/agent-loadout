//! Performance baseline (#10, docs/perf): times the commands the GUI calls
//! on startup, on page visits and on each drift trigger, against a
//! generated temp home with a large store (200 servers, 300 skills, 20
//! projects, ~450 commits) and client files holding foreign entries.
//! `RootedFs` keeps every path inside the temp root, as in the other
//! command tests. Ignored by default; run in release for real numbers:
//!
//! ```text
//! cargo test --release --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture tests_perf
//! ```
//!
//! `PERF_OUT` names a folder for `backend.json`.

use std::path::Path;
use std::time::{Duration, Instant};

use agent_loadout_core::adapters::ADAPTERS;
use agent_loadout_core::formats::Format;
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::store::Operation;

use super::history::{HistoryFilter, HistoryScope};
use super::skills::SkillScope;
use super::tests::Fx;
use super::{
    Core, apply, effective, history, project_servers, projects, remotes, servers, skills, tools,
};

const SERVERS: usize = 200;
const SKILLS: usize = 300;
const PROJECTS: usize = 20;
const EDITS: usize = 300;
const FOREIGN: usize = 25;
/// Runs per command; slow commands run fewer times (see [`runs_for`]).
const RUNS: usize = 7;

/// Runs for a command whose first run took `first`: the slowest commands
/// take tens of seconds on this dataset, and 7 runs of each would keep the
/// test busy for over an hour.
fn runs_for(first: Duration) -> usize {
    if first > Duration::from_secs(15) {
        1
    } else if first > Duration::from_secs(2) {
        3
    } else {
        RUNS
    }
}

/// Every other adapter (and the first four) gets a user file, as on a
/// machine with about half the clients installed.
fn installed(index: usize) -> bool {
    index < 4 || index.is_multiple_of(2)
}

fn foreign_json(keys: &[String]) -> String {
    let mut value = serde_json::Value::Object(
        (0..FOREIGN)
            .map(|n| {
                (
                    format!("foreign-{n}"),
                    serde_json::json!({ "command": "foreign-mcp", "args": ["--n", n.to_string()] }),
                )
            })
            .collect(),
    );
    for key in keys.iter().rev() {
        value = serde_json::json!({ key.as_str(): value });
    }
    serde_json::to_string_pretty(&value).unwrap()
}

fn foreign_toml(keys: &[String]) -> String {
    let table = keys
        .iter()
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>()
        .join(".");
    (0..FOREIGN)
        .map(|n| format!("[{table}.\"foreign-{n}\"]\ncommand = \"foreign-mcp\"\nargs = [\"--n\", \"{n}\"]\n\n"))
        .collect()
}

/// Writes the user MCP file of each installed client, with foreign entries.
fn client_files(core: &Core) -> Vec<&'static str> {
    let mut detected = Vec::new();
    for (index, spec) in ADAPTERS.iter().enumerate() {
        if !installed(index) {
            continue;
        }
        let Ok(targets) = spec.mcp_targets(
            &core.env,
            core.fs.as_ref(),
            TargetScope::User,
            None,
            &ClientPrefs::default(),
        ) else {
            continue;
        };
        let Some(target) = targets.first() else {
            continue;
        };
        let keys = target.container_keys().unwrap_or_default();
        let text = match target.format {
            Format::Json | Format::Jsonc => foreign_json(&keys),
            Format::Toml => foreign_toml(&keys),
            _ => target.skeleton.to_owned(),
        };
        std::fs::create_dir_all(target.file.parent().unwrap()).unwrap();
        std::fs::write(&target.file, text).unwrap();
        detected.push(spec.id);
    }
    detected
}

fn server_toml(i: usize, clients: &[&str], description: &str) -> String {
    let list = clients
        .iter()
        .enumerate()
        .filter(|(c, _)| !(i + c).is_multiple_of(4))
        .map(|(_, c)| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "schema = 1\nid = \"server-{i:03}\"\ndescription = \"{description}\"\ntransport = \"stdio\"\n\
         command = \"npx\"\nargs = [\"-y\", \"@perf/server-{i}\"]\n\n[env]\nLOG_LEVEL = \"info\"\n\n\
         [enable]\nclients = [{list}]\n"
    )
}

fn stage(core: &Core, files: &[(String, String)]) {
    let store = core.open_store_for_change().unwrap();
    store
        .transaction(Operation::new("test.edit", "perf: edit"), |tx| {
            files
                .iter()
                .try_for_each(|(path, text)| tx.write(path, text.clone()))
        })
        .unwrap();
}

fn skill_folder(root: &Path, i: usize) -> std::path::PathBuf {
    let dir = root.join("downloads").join(format!("skill-{i:03}"));
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: skill-{i:03}\ndescription: Synthetic skill {i} for the performance baseline.\n---\n\n# Skill {i}\n\n{}",
            "Some instructions for the agent.\n".repeat(40)
        ),
    )
    .unwrap();
    std::fs::write(dir.join("scripts").join("check.sh"), "echo ok\n").unwrap();
    dir
}

fn project_manifest(i: usize) -> String {
    let mut text = format!(
        "schema = 1\nid = \"proj-{i:02}-perf\"\nname = \"project-{i}\"\n\n[defaults]\nclients = [\"claude-code\", \"codex\"]\n"
    );
    for n in 0..5 {
        text.push_str(&format!(
            "\n[servers.local-{n}]\ntransport = \"stdio\"\ncommand = \"project-mcp-{n}\"\n"
        ));
    }
    text.push_str("\n[servers.server-001]\nfrom = \"server-001\"\nargs = [\"--project\"]\n");
    text
}

struct Timing {
    name: String,
    runs: usize,
    min: Duration,
    median: Duration,
    max: Duration,
    bytes: usize,
}

fn time<T: serde::Serialize>(out: &mut Vec<Timing>, name: &str, mut f: impl FnMut() -> T) {
    let mut runs = Vec::with_capacity(RUNS);
    let mut bytes = 0;
    let mut planned = RUNS;
    while runs.len() < planned {
        let start = Instant::now();
        let value = f();
        runs.push(start.elapsed());
        if runs.len() == 1 {
            planned = runs_for(runs[0]);
        }
        bytes = serde_json::to_vec(&value).map(|v| v.len()).unwrap_or(0);
    }
    runs.sort();
    let n = runs.len();
    // Printed as it goes: the whole run takes tens of minutes.
    eprintln!("timed {name}: median {} ms ({n} runs)", ms(runs[n / 2]));
    out.push(Timing {
        name: name.to_owned(),
        runs: n,
        min: runs[0],
        median: runs[n / 2],
        max: runs[n - 1],
        bytes,
    });
}

fn record(setup: &mut Vec<(&'static str, Duration)>, name: &'static str, started: Instant) {
    let elapsed = started.elapsed();
    eprintln!("setup {name}: {} ms", ms(elapsed));
    setup.push((name, elapsed));
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

#[test]
#[ignore = "performance baseline; run on demand (docs/perf)"]
fn perf_baseline_commands() {
    let fx = Fx::new();
    let core = &fx.core;
    drop(fx.store());
    let mut setup = Vec::new();

    let started = Instant::now();
    let detected = client_files(core);
    record(&mut setup, "client files", started);

    let started = Instant::now();
    let files: Vec<(String, String)> = (0..SERVERS)
        .map(|i| {
            (
                format!("servers/server-{i:03}.toml"),
                server_toml(i, &detected, "initial"),
            )
        })
        .collect();
    stage(core, &files);
    record(&mut setup, "stage 200 servers (one commit)", started);

    let started = Instant::now();
    let first = apply::pending(core).unwrap();
    record(&mut setup, "first pending plan (200 servers x clients)", started);
    eprintln!(
        "first plan: {} files, {} pending, {} writes",
        first.files.len(),
        first.pending,
        first.writes
    );

    let started = Instant::now();
    apply::run(core, None).unwrap();
    record(&mut setup, "first apply (all client files)", started);

    let started = Instant::now();
    for e in 0..EDITS {
        let i = e % SERVERS;
        stage(
            core,
            &[(
                format!("servers/server-{i:03}.toml"),
                server_toml(i, &detected, &format!("edit {e}")),
            )],
        );
    }
    apply::run(core, None).unwrap();
    record(&mut setup, "300 edit commits + apply", started);

    let started = Instant::now();
    for i in 0..SKILLS {
        let folder = skill_folder(&fx.th.projects(), i);
        let step = Instant::now();
        skills::install_local(core, &folder, None, false).unwrap();
        if i % 100 == 0 || i == SKILLS - 1 {
            eprintln!("skill {i}: install {} ms", ms(step.elapsed()));
        }
    }
    record(&mut setup, "install 300 local skills", started);

    let started = Instant::now();
    let skill_clients: Vec<String> = detected.iter().take(3).map(|c| (*c).to_owned()).collect();
    for i in (0..SKILLS).step_by(3) {
        let step = Instant::now();
        skills::set_clients(core, &format!("skill-{i:03}"), &skill_clients, true, None).unwrap();
        if i % 99 == 0 {
            eprintln!("skill {i}: enable for 3 clients {} ms", ms(step.elapsed()));
        }
    }
    record(&mut setup, "enable 100 skills for 3 clients", started);

    let started = Instant::now();
    let mut project_ids = Vec::new();
    for i in 0..PROJECTS {
        let root = fx.th.projects().join(format!("project-{i}"));
        std::fs::create_dir_all(root.join(".loadout")).unwrap();
        std::fs::write(root.join(".loadout/project.toml"), project_manifest(i)).unwrap();
        let step = Instant::now();
        let added = projects::add(core, &root).unwrap();
        let add_ms = ms(step.elapsed());
        let step = Instant::now();
        if i % 5 != 4 {
            let hash = projects::review(core, &added.id).unwrap().hash;
            projects::trust(core, &added.id, &hash).unwrap();
        }
        eprintln!(
            "project {i}: add {add_ms} ms, review + trust {} ms",
            ms(step.elapsed())
        );
        project_ids.push(added.id);
    }
    apply::run(core, None).unwrap();
    record(&mut setup, "add 20 projects (16 trusted) + apply", started);

    let commits = fx.commits();
    let project = project_ids[0].clone();
    let user = SkillScope::User;
    let in_project = SkillScope::Project {
        id: project.clone(),
    };
    let all = HistoryFilter {
        scope: HistoryScope::All,
        entity: None,
        limit: None,
    };

    let mut out = Vec::new();
    // Startup (Skills is the first page) and every drift trigger.
    time(&mut out, "list_projects", || projects::list(core).unwrap());
    time(&mut out, "pending_plan", || apply::pending(core).unwrap());
    time(&mut out, "list_skills (user)", || {
        skills::list(core, &user).unwrap()
    });
    time(&mut out, "list_skill_tool_dirs (user)", || {
        skills::tool_dirs(core, &user).unwrap()
    });
    time(&mut out, "remotes_overview", || {
        remotes::overview(core).unwrap()
    });
    // Page visits.
    time(&mut out, "list_servers", || servers::list(core).unwrap());
    time(&mut out, "list_tools", || tools::list(core).unwrap());
    time(&mut out, "list_history (all, no limit)", || {
        history::history(core, &all).unwrap()
    });
    time(&mut out, "list_tags", || history::tags(core).unwrap());
    time(&mut out, "list_merge_conflicts", || {
        history::merge_conflicts(core).unwrap()
    });
    time(&mut out, "list_applied_runs", || {
        apply::applied_runs(core).unwrap()
    });
    time(&mut out, "pending_store_changes", || {
        apply::store_changes(core).unwrap()
    });
    time(&mut out, "list_skills (project)", || {
        skills::list(core, &in_project).unwrap()
    });
    time(&mut out, "list_skill_tool_dirs (project)", || {
        skills::tool_dirs(core, &in_project).unwrap()
    });
    time(&mut out, "list_project_servers", || {
        project_servers::list(core, &project).unwrap()
    });
    time(&mut out, "effective_matrix", || {
        effective::matrix(core, &project).unwrap()
    });
    // What one window focus costs on the MCP page: the GUI's refetch set.
    time(
        &mut out,
        "focus on MCP (pending_plan + list_servers + list_projects)",
        || {
            (
                apply::pending(core).unwrap(),
                servers::list(core).unwrap(),
                projects::list(core).unwrap(),
            )
        },
    );
    time(
        &mut out,
        "focus on Skills (pending_plan + list_projects + list_skills + tool_dirs)",
        || {
            (
                apply::pending(core).unwrap(),
                projects::list(core).unwrap(),
                skills::list(core, &user).unwrap(),
                skills::tool_dirs(core, &user).unwrap(),
            )
        },
    );
    // A single pill toggle (instant mode: store commit + apply of that client).
    let mut toggle = 0usize;
    time(
        &mut out,
        "set_server_clients (one pill, instant apply)",
        || {
            toggle += 1;
            let clients = if toggle % 2 == 1 {
                vec![detected[0].to_owned()]
            } else {
                detected.iter().map(|c| (*c).to_owned()).collect()
            };
            servers::set_clients_for(
                core,
                "server-010",
                &servers::ClientChoice {
                    all_detected: false,
                    clients,
                },
            )
            .unwrap()
        },
    );

    eprintln!(
        "\nstore: {SERVERS} servers, {SKILLS} skills, {PROJECTS} projects, {commits} commits, {} clients with files",
        detected.len()
    );
    eprintln!("\n| Setup step | ms |\n|---|---|");
    for (name, d) in &setup {
        eprintln!("| {name} | {} |", ms(*d));
    }
    eprintln!(
        "\n| Command | runs | min ms | median ms | max ms | JSON bytes |\n|---|---|---|---|---|---|"
    );
    for t in &out {
        eprintln!(
            "| {} | {} | {} | {} | {} | {} |",
            t.name,
            t.runs,
            ms(t.min),
            ms(t.median),
            ms(t.max),
            t.bytes
        );
    }
    if let Some(dir) = std::env::var_os("PERF_OUT") {
        let json: Vec<_> = out
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "runs": t.runs,
                    "minMs": ms(t.min),
                    "medianMs": ms(t.median),
                    "maxMs": ms(t.max),
                    "bytes": t.bytes,
                })
            })
            .collect();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("backend.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }
}
