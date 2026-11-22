//! `docs/adapters.md` stays in step with the adapter table: the block
//! between `<!-- adapters:begin -->` and `<!-- adapters:end -->` is rendered
//! from `ADAPTERS` (and the skill folders of `CLIENT_SKILLS`), and every
//! adapter id has a row in the hand-written verification table.
//!
//! Set `LOADOUT_UPDATE_DOCS=1` to rewrite the generated block in place.

use std::path::{Path, PathBuf};

use agent_loadout_core::Os;
use agent_loadout_core::adapters::caps::{DisableMech, ProjectDisable, TransportSet};
use agent_loadout_core::adapters::location::Location;
use agent_loadout_core::adapters::{ADAPTERS, AdapterSpec, ContainerPath, McpSpec, Tier};
use agent_loadout_core::skills::link::client_skills;

const BEGIN: &str = "<!-- adapters:begin -->";
const END: &str = "<!-- adapters:end -->";
const STATUS_HEADING: &str = "## Verification status";

fn doc_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("adapters.md")
}

/// `base/rel`, leaving out an empty part.
fn join(base: &str, rel: &str) -> String {
    match (base.is_empty(), rel.is_empty()) {
        (_, true) => base.to_owned(),
        (true, false) => rel.to_owned(),
        (false, false) => format!("{base}/{rel}"),
    }
}

/// A location in the notation of the legend in `docs/adapters.md`.
fn location(loc: &Location) -> String {
    match *loc {
        Location::Home(rel) => join("~", rel),
        Location::AppData(rel) => join("<AppData>", rel),
        Location::LocalAppData(rel) => join("%LOCALAPPDATA%", rel),
        Location::XdgConfigAllOs(rel) => join("$XDG_CONFIG_HOME", rel),
        Location::EnvDir {
            var, fallback, rel, ..
        } => join(&format!("${{{var}:-{}}}", location(fallback)), rel),
        Location::HomeNamedDir { var, default, rel } => {
            join(&format!("~/${{{var}:-{default}}}"), rel)
        }
        Location::Project(rel) => join("<P>", rel),
        Location::Glob { base, pattern } => join(&location(base), pattern),
    }
}

fn container(path: ContainerPath) -> String {
    path.segments()
        .iter()
        .map(|segment| {
            if *segment == ContainerPath::PROJECT_KEY {
                "<P>"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Candidate files, first existing one wins.
fn candidates(locations: &[Location]) -> String {
    locations
        .iter()
        .map(|loc| format!("`{}`", location(loc)))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn tier(tier: Tier) -> &'static str {
    match tier {
        Tier::A => "A",
        Tier::B => "B",
        Tier::C => "C",
        Tier::D => "D",
    }
}

fn oses(oses: &[Os]) -> String {
    if [Os::Windows, Os::Linux, Os::MacOs]
        .iter()
        .all(|os| oses.contains(os))
    {
        return "all".to_owned();
    }
    oses.iter()
        .map(|os| os.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn transports(set: TransportSet) -> String {
    [(set.stdio, "stdio"), (set.http, "http"), (set.sse, "sse")]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn native_disable(mech: Option<DisableMech>) -> String {
    match mech {
        Some(DisableMech::EnabledFalse) => "`enabled: false`".to_owned(),
        Some(DisableMech::DisabledTrue) => "`disabled: true`".to_owned(),
        Some(DisableMech::ListMember(path)) => format!("listed in `{}`", path.join(".")),
        None => "none".to_owned(),
    }
}

fn project_disable(mcp: &McpSpec) -> String {
    match mcp.caps.project_disable {
        Some(ProjectDisable::ListMember(list)) => format!("listed in `{}`", container(list)),
        Some(ProjectDisable::EnabledFalse { partial: true }) => {
            "`{ enabled: false }` entry".to_owned()
        }
        Some(ProjectDisable::EnabledFalse { partial: false }) => {
            "full entry with `enabled: false`".to_owned()
        }
        None => "none".to_owned(),
    }
}

fn user_files(mcp: &McpSpec) -> String {
    let mut cell = candidates(mcp.user);
    if mcp.user_files.is_some() {
        cell.push_str(" (computed, see notes)");
    }
    if let Some(profiles) = &mcp.profiles {
        cell.push_str(&format!(
            "; profiles: `{}`",
            join(&join(&location(&profiles.dir), "<name>"), profiles.rel)
        ));
    }
    for mirror in mcp.user_mirrors {
        cell.push_str(&format!("; mirror: `{}`", location(mirror)));
    }
    cell
}

fn project_files(mcp: &McpSpec) -> String {
    if mcp.project.is_empty() {
        "none".to_owned()
    } else {
        candidates(mcp.project)
    }
}

fn project_local_files(mcp: &McpSpec) -> String {
    if mcp.project_local.is_empty() {
        return "none".to_owned();
    }
    let mut cell = candidates(mcp.project_local);
    if mcp.project_local_container != mcp.container {
        cell.push_str(&format!(" at `{}`", container(mcp.project_local_container)));
    }
    cell
}

fn skills(spec: &AdapterSpec) -> String {
    match client_skills(spec.id) {
        Some(entry) => format!(
            "`{}`, `{}`",
            location(&entry.user),
            join("<P>", entry.project)
        ),
        None => "none".to_owned(),
    }
}

fn row(spec: &AdapterSpec) -> String {
    let Some(mcp) = &spec.mcp else {
        return format!(
            "| `{}` | {} | {} | none | none | none | none | none | none | none | none | {} |",
            spec.id,
            spec.display_name,
            tier(spec.tier),
            skills(spec)
        );
    };
    let mut tier_cell = tier(spec.tier).to_owned();
    if mcp.read_only {
        tier_cell.push_str(", read only");
    }
    let format = format!("{} `{}`", mcp.format.as_str(), container(mcp.container));
    [
        format!("`{}`", spec.id),
        spec.display_name.to_owned(),
        tier_cell,
        oses(mcp.oses),
        format,
        user_files(mcp),
        project_files(mcp),
        project_local_files(mcp),
        transports(mcp.caps.transports),
        native_disable(mcp.caps.native_disable),
        project_disable(mcp),
        skills(spec),
    ]
    .iter()
    .fold(String::from("|"), |line, cell| format!("{line} {cell} |"))
}

fn render() -> String {
    let mut out = String::from(
        "| Id | Name | Tier | OS | Format and container | User file | Project file | \
         Project-local file | Transports | Native disable | Hide a user server in a project | \
         Skill folders (user, project) |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for spec in ADAPTERS {
        out.push_str(&row(spec));
        out.push('\n');
    }
    out
}

#[test]
fn adapter_table_matches_the_code() {
    let path = doc_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .replace("\r\n", "\n");
    let start = text
        .find(BEGIN)
        .unwrap_or_else(|| panic!("{BEGIN} not found in {}", path.display()))
        + BEGIN.len();
    let end = text
        .find(END)
        .unwrap_or_else(|| panic!("{END} not found in {}", path.display()));
    assert!(start <= end, "{END} comes before {BEGIN}");
    let generated = format!("\n{}", render());
    if std::env::var("LOADOUT_UPDATE_DOCS").as_deref() == Ok("1") {
        let updated = format!("{}{generated}{}", &text[..start], &text[end..]);
        std::fs::write(&path, updated).unwrap();
        return;
    }
    assert_eq!(
        text[start..end],
        generated,
        "docs/adapters.md is stale; rerun with LOADOUT_UPDATE_DOCS=1"
    );

    // Every adapter has a row in the hand-written verification table, which
    // follows the generated block.
    let status = text[end..]
        .find(STATUS_HEADING)
        .map(|at| &text[end + at..])
        .unwrap_or_else(|| panic!("{STATUS_HEADING:?} not found after {END}"));
    for spec in ADAPTERS {
        assert!(
            status.contains(&format!("| `{}` |", spec.id)),
            "docs/adapters.md has no verification row for `{}`",
            spec.id
        );
    }
}
