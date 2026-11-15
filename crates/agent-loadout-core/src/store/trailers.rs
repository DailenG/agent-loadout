//! `Loadout-*` git trailers on store commits (SPEC 9.2):
//!
//! ```text
//! server: update github (env.GITHUB_TOOLSETS)
//!
//! Loadout-Op: server.update
//! Loadout-Entity: server/github
//! Loadout-Scope: system
//! Loadout-Machine: <machine-id> (<label>)
//! Loadout-Client: loadout-cli/0.1.0
//! Loadout-Apply: <apply-id> ok
//! ```
//!
//! A bulk change lists one `Loadout-Entity` per entity. Unknown `Loadout-*`
//! trailers from a newer build are kept in [`Trailers::other`].

use std::fmt;

use crate::brand::TRAILER_PREFIX;
use crate::model::applied::ApplyOutcome;

const OP: &str = "Op";
const ENTITY: &str = "Entity";
const SCOPE: &str = "Scope";
const MACHINE: &str = "Machine";
const CLIENT: &str = "Client";
const APPLY: &str = "Apply";

/// The layer a commit changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// User-wide desired state (`system`).
    System,
    /// One project (`project/<id>`).
    Project(String),
    /// A value this build does not know, kept as written.
    Other(String),
}

impl Scope {
    pub fn parse(text: &str) -> Self {
        if text == "system" {
            return Self::System;
        }
        match text.split_once('/') {
            Some(("project", id)) if !id.is_empty() => Self::Project(id.to_owned()),
            _ => Self::Other(text.to_owned()),
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System => f.write_str("system"),
            Self::Project(id) => write!(f, "project/{id}"),
            Self::Other(text) => f.write_str(text),
        }
    }
}

/// `Loadout-Machine: <machine-id> (<label>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRef {
    pub id: String,
    pub label: String,
}

/// `Loadout-Apply: <apply-id> <outcome>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyRef {
    pub id: String,
    pub outcome: ApplyOutcome,
}

/// The `Loadout-*` trailers of one commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trailers {
    /// Operation (`server.add`, `store.init`).
    pub op: Option<String>,
    /// Changed entities (`server/github`), one per trailer line.
    pub entities: Vec<String>,
    pub scope: Option<Scope>,
    pub machine: Option<MachineRef>,
    /// Program and version that made the change (`loadout-cli/0.1.0`).
    pub client: Option<String>,
    pub apply: Option<ApplyRef>,
    /// Other `Loadout-*` trailers in order, keys without the prefix.
    pub other: Vec<(String, String)>,
}

impl Trailers {
    /// Trailer lines (`Loadout-Op: server.add`), each ending in `\n`.
    /// Line breaks inside values become spaces.
    pub fn format(&self) -> String {
        let mut lines: Vec<(&str, String)> = Vec::new();
        if let Some(op) = &self.op {
            lines.push((OP, op.clone()));
        }
        for entity in &self.entities {
            lines.push((ENTITY, entity.clone()));
        }
        if let Some(scope) = &self.scope {
            lines.push((SCOPE, scope.to_string()));
        }
        if let Some(machine) = &self.machine {
            lines.push((MACHINE, format!("{} ({})", machine.id, machine.label)));
        }
        if let Some(client) = &self.client {
            lines.push((CLIENT, client.clone()));
        }
        if let Some(apply) = &self.apply {
            lines.push((
                APPLY,
                format!("{} {}", apply.id, outcome_str(apply.outcome)),
            ));
        }
        for (key, value) in &self.other {
            lines.push((key, value.clone()));
        }
        lines
            .into_iter()
            .map(|(key, value)| format!("{TRAILER_PREFIX}{key}: {}\n", one_line(&value)))
            .collect()
    }

    /// Reads the `Loadout-*` trailers from the last paragraph of a commit
    /// message. Other trailers are ignored; a message without a trailer
    /// paragraph yields empty trailers.
    pub fn parse(message: &str) -> Self {
        let mut trailers = Self::default();
        for (key, value) in trailer_block(message) {
            let Some(key) = key.strip_prefix(TRAILER_PREFIX) else {
                continue;
            };
            match key {
                OP if trailers.op.is_none() => trailers.op = Some(value.to_owned()),
                ENTITY => trailers.entities.push(value.to_owned()),
                SCOPE if trailers.scope.is_none() => trailers.scope = Some(Scope::parse(value)),
                MACHINE if trailers.machine.is_none() => {
                    trailers.machine = Some(parse_machine(value));
                }
                CLIENT if trailers.client.is_none() => trailers.client = Some(value.to_owned()),
                APPLY if trailers.apply.is_none() && parse_apply(value).is_some() => {
                    trailers.apply = parse_apply(value);
                }
                _ => trailers.other.push((key.to_owned(), value.to_owned())),
            }
        }
        trailers
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A full commit message: `summary`, a blank line and the trailers.
pub fn compose_message(summary: &str, trailers: &Trailers) -> String {
    let summary = one_line(summary);
    if trailers.is_empty() {
        format!("{summary}\n")
    } else {
        format!("{summary}\n\n{}", trailers.format())
    }
}

/// The first line of a commit message.
pub fn message_summary(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim_end()
}

fn one_line(value: &str) -> String {
    value
        .split(['\r', '\n'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

const fn outcome_str(outcome: ApplyOutcome) -> &'static str {
    match outcome {
        ApplyOutcome::Ok => "ok",
        ApplyOutcome::Partial => "partial",
        ApplyOutcome::Blocked => "blocked",
        ApplyOutcome::Failed => "failed",
    }
}

fn parse_machine(value: &str) -> MachineRef {
    let (id, rest) = value.split_once(' ').unwrap_or((value, ""));
    let rest = rest.trim();
    let label = rest
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(rest);
    MachineRef {
        id: id.to_owned(),
        label: label.to_owned(),
    }
}

fn parse_apply(value: &str) -> Option<ApplyRef> {
    let (id, outcome) = value.split_once(' ')?;
    let outcome = [
        ApplyOutcome::Ok,
        ApplyOutcome::Partial,
        ApplyOutcome::Blocked,
        ApplyOutcome::Failed,
    ]
    .into_iter()
    .find(|candidate| outcome_str(*candidate) == outcome.trim())?;
    Some(ApplyRef {
        id: id.to_owned(),
        outcome,
    })
}

/// `(key, value)` pairs of the trailer paragraph: the last paragraph, when it
/// is not the summary and every line is `Token: value`.
fn trailer_block(message: &str) -> Vec<(&str, &str)> {
    let lines: Vec<&str> = message.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |i| i + 1);
    let start = lines[..end]
        .iter()
        .rposition(|line| line.is_empty())
        .map_or(0, |i| i + 1);
    if start == 0 {
        return Vec::new();
    }
    lines[start..end]
        .iter()
        .map(|line| {
            let (key, value) = line.split_once(':')?;
            let token =
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            token.then(|| (key, value.trim()))
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}
