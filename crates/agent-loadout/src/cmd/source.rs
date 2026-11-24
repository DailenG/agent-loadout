//! `loadout source ls|trust|block|untrust` (#24, SPEC 14 and 17), and the
//! trust prompt `skills add` and `skills update` show for a source the
//! user has not trusted.
//!
//! A trust decision is never an agent's: an interactive terminal prompts,
//! and a command without one needs the explicit `--trust-source` flag.
//! `--yes` never answers the trust prompt.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};

use agent_loadout_core::sources::{
    self, AskReason, LapseReason, SourceDecision, SourceRecord, SourceRef, SourceScope, TrustCheck,
    TrustGrant, TrustState, TrustVia,
};
use agent_loadout_core::store::Store;
use agent_loadout_core::{CoreError, ErrorCode, brand};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, print_json};

/// The warning shown wherever content comes from outside (#24).
pub const WARNING: &str = "Only install from sources you trust. Skills and MCP servers can run commands and read your files.";

#[derive(Debug, Args)]
pub struct SourceArgs {
    #[command(subcommand)]
    command: SourceCommand,
}

#[derive(Debug, Subcommand)]
enum SourceCommand {
    /// List trusted and blocked sources.
    Ls,
    /// Always trust a source: a git URL, or a source key (host/owner/repo,
    /// host/owner/*, host/*).
    Trust {
        #[arg(value_name = "URL|KEY")]
        source: String,
        /// What a URL's trust covers [default: repo].
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    /// Never trust a source (a block wins over any trust).
    Block {
        #[arg(value_name = "URL|KEY")]
        source: String,
        /// What a URL's block covers [default: repo].
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    /// Remove the trust or block of a source key. Nothing is uninstalled;
    /// the next install or update from it asks again.
    Untrust {
        #[arg(value_name = "KEY")]
        key: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ScopeArg {
    Repo,
    Owner,
    Host,
}

impl From<ScopeArg> for SourceScope {
    fn from(scope: ScopeArg) -> Self {
        match scope {
            ScopeArg::Repo => SourceScope::Repo,
            ScopeArg::Owner => SourceScope::Owner,
            ScopeArg::Host => SourceScope::Host,
        }
    }
}

/// `--trust-source[=once|repo|owner|host]` on `skills add` and `skills
/// update`: the user's answer to the trust prompt, given up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TrustSourceArg {
    /// Install or update this time only.
    Once,
    /// Always trust this repository.
    Repo,
    /// Always trust every repository of this owner.
    Owner,
    /// Always trust every repository on this host (not for public hosts
    /// such as github.com).
    Host,
}

impl TrustSourceArg {
    pub fn grant(self) -> TrustGrant {
        match self {
            TrustSourceArg::Once => TrustGrant::Once,
            TrustSourceArg::Repo => TrustGrant::Always(SourceScope::Repo),
            TrustSourceArg::Owner => TrustGrant::Always(SourceScope::Owner),
            TrustSourceArg::Host => TrustGrant::Always(SourceScope::Host),
        }
    }
}

pub fn run(ctx: &Context, args: &SourceArgs) -> Result<u8, CoreError> {
    match &args.command {
        SourceCommand::Ls => list(ctx),
        SourceCommand::Trust { source, scope } => {
            decide_cmd(ctx, source, *scope, SourceDecision::Trust)
        }
        SourceCommand::Block { source, scope } => {
            decide_cmd(ctx, source, *scope, SourceDecision::Block)
        }
        SourceCommand::Untrust { key } => untrust(ctx, key),
    }
}

fn record_json(key: &str, record: &SourceRecord) -> Value {
    json!({
        "key": key,
        "decision": record.decision.as_str(),
        "scope": record.scope.as_str(),
        "decided_at": record.decided_at.to_string(),
        "via": record.via.map(TrustVia::as_str),
        "machine": record.machine,
        "url": record.url,
        "lapsed": record.lapsed.as_ref().map(|lapse| json!({
            "reason": lapse.reason.as_str(),
            "at": lapse.at.to_string(),
            "detail": lapse.detail,
        })),
        "official": is_official(key),
    })
}

fn is_official(key: &str) -> bool {
    sources::OFFICIAL_OWNERS
        .iter()
        .any(|owner| key == format!("{owner}/*") || key.starts_with(&format!("{owner}/")))
}

fn lapse_text(reason: LapseReason, detail: Option<&str>) -> String {
    match reason {
        LapseReason::HistoryRewritten => "history rewritten (force-push)".to_owned(),
        LapseReason::Moved => format!("moved to {}", detail.unwrap_or("another location")),
    }
}

fn list(ctx: &Context) -> Result<u8, CoreError> {
    let records = match ctx.open_store()? {
        Some(store) => sources::list(&store)?,
        None => Vec::new(),
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "sources": records
                .iter()
                .map(|(key, record)| record_json(key, record))
                .collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    if records.is_empty() {
        println!("No trusted or blocked sources.");
        return Ok(0);
    }
    for (key, record) in &records {
        let state = match (&record.decision, &record.lapsed) {
            (SourceDecision::Block, _) => "blocked".to_owned(),
            (SourceDecision::Trust, Some(lapse)) => {
                format!(
                    "trust lapsed: {}",
                    lapse_text(lapse.reason, lapse.detail.as_deref())
                )
            }
            (SourceDecision::Trust, None) => "trusted".to_owned(),
        };
        let via = record
            .via
            .map(|via| format!(" via {}", via.as_str()))
            .unwrap_or_default();
        let machine = record
            .machine
            .as_deref()
            .map(|machine| format!(" on {machine}"))
            .unwrap_or_default();
        let official = if is_official(key) { "  (official)" } else { "" };
        println!(
            "{key:<48} {state:<10} {:<5} {}{via}{machine}{official}",
            record.scope.as_str(),
            record.decided_at
        );
    }
    Ok(0)
}

/// A URL or path argument (rather than a source key).
fn is_url(text: &str) -> bool {
    text.contains("://")
        || text.contains('@')
        || text.contains('\\')
        || std::path::Path::new(text).is_absolute()
        || text.starts_with('/')
}

/// Trust or block a URL (at `--scope`, the repository by default) or a
/// source key. Either goes through the parser source keys come from, so
/// the recorded key is the one a URL of that source matches
/// (`github.com/Acme/*` is recorded as `github.com/acme/*`).
fn decide_cmd(
    ctx: &Context,
    text: &str,
    scope: Option<ScopeArg>,
    decision: SourceDecision,
) -> Result<u8, CoreError> {
    let text = text.trim();
    let wanted = scope.map(SourceScope::from);
    let (key, repo) = if text.ends_with("/*") {
        let (key, shape) = SourceRef::canonical_key(text)?;
        if let Some(wanted) = wanted
            && wanted != shape
        {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "{key} is a{} {} key; --scope {} does not fit it",
                    if shape == SourceScope::Owner { "n" } else { "" },
                    shape.as_str(),
                    wanted.as_str()
                ),
            ));
        }
        (key, None)
    } else {
        let source = SourceRef::from_key_or_url(text)?;
        let scope = wanted.unwrap_or(SourceScope::Repo);
        let key = source.scope_key(scope).ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("{} has no {} scope", source.key(), scope.as_str()),
            )
        })?;
        (key, Some((source, scope)))
    };
    let store = ctx.open_store_for_change()?;
    let via = TrustVia::CliCommand;
    let commit = match repo {
        // A URL is shown with a repository record.
        Some((source, scope)) if is_url(text) => match decision {
            SourceDecision::Trust => sources::trust(&store, &source, scope, via)?,
            SourceDecision::Block => sources::block(&store, &source, scope, via)?,
        },
        _ => sources::record_key(&store, &key, decision, via, None)?,
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "key": key,
            "decision": decision.as_str(),
            "commit": commit.as_ref().map(ToString::to_string),
        }));
    } else {
        match decision {
            SourceDecision::Trust => println!("Trusted {key}."),
            SourceDecision::Block => println!("Blocked {key}."),
        }
    }
    Ok(0)
}

fn untrust(ctx: &Context, key: &str) -> Result<u8, CoreError> {
    let (key, _) = SourceRef::canonical_key(key)?;
    let key = key.as_str();
    let store = ctx.open_store_for_change()?;
    let commit = sources::untrust(&store, key)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "key": key,
            "removed": true,
            "commit": commit.as_ref().map(ToString::to_string),
        }));
    } else {
        println!(
            "Removed the decision about {key}; the next install or update from it asks again."
        );
    }
    Ok(0)
}

/// The user's answer for one source, or none when the store already
/// decides (trusted: go ahead; blocked: the gate refuses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    NotNeeded,
    Grant(TrustGrant, TrustVia),
    Cancelled,
}

impl Answer {
    pub fn grant(self) -> (Option<TrustGrant>, TrustVia) {
        match self {
            Answer::Grant(grant, via) => (Some(grant), via),
            Answer::NotNeeded | Answer::Cancelled => (None, TrustVia::CliPrompt),
        }
    }
}

/// Answers for several sources in one run (`skills update` of every
/// skill): each source is asked once.
#[derive(Debug, Default)]
pub struct Answers {
    by_key: BTreeMap<String, Answer>,
}

impl Answers {
    /// The answer for `source`: the `--trust-source` flag, else the store's
    /// decision, else (in a terminal) the prompt. Without a terminal, or
    /// with `--json`, an undecided source fails with
    /// `source_trust_required`.
    pub fn decide(
        &mut self,
        ctx: &Context,
        store: Option<&Store<'_>>,
        source: &SourceRef,
        flag: Option<TrustSourceArg>,
    ) -> Result<Answer, CoreError> {
        if let Some(flag) = flag {
            return Ok(Answer::Grant(flag.grant(), TrustVia::CliFlag));
        }
        if let Some(answer) = self.by_key.get(source.key()) {
            return Ok(*answer);
        }
        let check = match store {
            Some(store) => sources::check(store, source)?,
            None => TrustCheck {
                source: source.clone(),
                state: TrustState::Ask {
                    reason: AskReason::FirstTime,
                    lapse: None,
                },
                scopes: source.scopes(),
                official: source.is_official(),
                installed_skills: Vec::new(),
            },
        };
        let answer = match &check.state {
            TrustState::Trusted { .. } | TrustState::Blocked { .. } => Answer::NotNeeded,
            TrustState::Ask { .. } => {
                let interactive =
                    !ctx.json && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
                if !interactive {
                    return Err(CoreError::new(
                        ErrorCode::SourceTrustRequired,
                        format!(
                            "the source {} is not trusted; run the command in a terminal to answer \
                             the trust prompt, or pass --trust-source[=once|repo|owner|host] \
                             (only for a source you trust: {})",
                            source.key(),
                            WARNING.to_lowercase()
                        ),
                    ));
                }
                let stdin = std::io::stdin();
                prompt(&check, &mut stdin.lock(), &mut std::io::stderr())?
            }
        };
        self.by_key.insert(source.key().to_owned(), answer);
        Ok(answer)
    }
}

fn read_answer(input: &mut dyn BufRead) -> Result<String, CoreError> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|err| CoreError::new(ErrorCode::IoError, format!("cannot read stdin: {err}")))?;
    Ok(line.trim().to_ascii_lowercase())
}

fn io(err: std::io::Error) -> CoreError {
    CoreError::new(
        ErrorCode::IoError,
        format!("cannot write the prompt: {err}"),
    )
}

/// The trust prompt: once, always (then the scope, the repository by
/// default) or cancel. Anything else, or end of input, cancels.
pub fn prompt(
    check: &TrustCheck,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Answer, CoreError> {
    let source = &check.source;
    let why = match &check.state {
        TrustState::Ask {
            reason: AskReason::Lapsed,
            lapse,
        } => match lapse {
            Some(lapse) => format!(
                "trusted before, but {} since",
                lapse_text(lapse.reason, lapse.detail.as_deref())
            ),
            None => "trusted before, but it changed since".to_owned(),
        },
        TrustState::Ask {
            reason: AskReason::NotTrusted,
            ..
        } => "not trusted".to_owned(),
        _ => "first time installing from it".to_owned(),
    };
    writeln!(out, "Do you trust this source?").map_err(io)?;
    writeln!(out, "  {} ({why})", source.key()).map_err(io)?;
    if check.official {
        writeln!(out, "  Official source.").map_err(io)?;
    }
    if source.is_insecure() {
        writeln!(
            out,
            "  Unencrypted connection (http:// or git://): anyone on the network path can change \
             what is downloaded. Use an https:// or SSH URL if the host has one."
        )
        .map_err(io)?;
    }
    if !check.installed_skills.is_empty() {
        writeln!(
            out,
            "  Installed from it: {}",
            check.installed_skills.join(", ")
        )
        .map_err(io)?;
    }
    writeln!(out, "  {WARNING}").map_err(io)?;
    write!(
        out,
        "[o] Trust this time only  [a] Always trust this source  [c] Cancel: "
    )
    .map_err(io)?;
    out.flush().map_err(io)?;
    match read_answer(input)?.as_str() {
        "o" | "once" => Ok(Answer::Grant(TrustGrant::Once, TrustVia::CliPrompt)),
        "a" | "always" => {
            let scope = if check.scopes.len() > 1 {
                writeln!(out, "Always trust:").map_err(io)?;
                for (index, scope) in check.scopes.iter().enumerate() {
                    let key = source.scope_key(*scope).unwrap_or_default();
                    let default = if index == 0 { " (default)" } else { "" };
                    writeln!(out, "  [{}] {key}{default}", index + 1).map_err(io)?;
                }
                write!(out, "Choice: ").map_err(io)?;
                out.flush().map_err(io)?;
                let answer = read_answer(input)?;
                if answer.is_empty() {
                    check.scopes[0]
                } else {
                    match answer
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|n| check.scopes.get(n))
                    {
                        Some(scope) => *scope,
                        None => return Ok(Answer::Cancelled),
                    }
                }
            } else {
                SourceScope::Repo
            };
            Ok(Answer::Grant(
                TrustGrant::Always(scope),
                TrustVia::CliPrompt,
            ))
        }
        _ => Ok(Answer::Cancelled),
    }
}

/// The error a cancelled prompt ends the command with.
pub fn cancelled(source: &SourceRef) -> CoreError {
    CoreError::new(
        ErrorCode::SourceTrustRequired,
        format!(
            "cancelled: nothing from {} was installed or updated; `{} source trust` records a \
             decision",
            source.key(),
            brand::BIN_NAME
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(url: &str) -> TrustCheck {
        let source = SourceRef::from_git_url(url).unwrap();
        TrustCheck {
            scopes: source.scopes(),
            official: source.is_official(),
            source,
            state: TrustState::Ask {
                reason: AskReason::FirstTime,
                lapse: None,
            },
            installed_skills: Vec::new(),
        }
    }

    fn answer(url: &str, typed: &str) -> (Answer, String) {
        let mut out = Vec::new();
        let answer = prompt(&check(url), &mut typed.as_bytes(), &mut out).unwrap();
        (answer, String::from_utf8(out).unwrap())
    }

    #[test]
    fn the_prompt_offers_once_always_and_cancel() {
        let (once, text) = answer("https://github.com/acme/skills", "o\n");
        assert_eq!(once, Answer::Grant(TrustGrant::Once, TrustVia::CliPrompt));
        assert!(
            text.contains("github.com/acme/skills (first time installing from it)"),
            "{text}"
        );
        assert!(text.contains(WARNING), "{text}");
        assert_eq!(
            answer("https://github.com/acme/skills", "a\n\n").0,
            Answer::Grant(TrustGrant::Always(SourceScope::Repo), TrustVia::CliPrompt)
        );
        let (owner, text) = answer("https://github.com/acme/skills", "a\n2\n");
        assert_eq!(
            owner,
            Answer::Grant(TrustGrant::Always(SourceScope::Owner), TrustVia::CliPrompt)
        );
        assert!(text.contains("[2] github.com/acme/*"), "{text}");
        assert!(!text.contains("github.com/*\n"), "{text}");
        assert_eq!(
            answer("https://github.com/acme/skills", "a\n9\n").0,
            Answer::Cancelled
        );
        assert_eq!(
            answer("https://github.com/acme/skills", "c\n").0,
            Answer::Cancelled
        );
        assert_eq!(
            answer("https://github.com/acme/skills", "yes\n").0,
            Answer::Cancelled
        );
        assert_eq!(
            answer("https://github.com/acme/skills", "").0,
            Answer::Cancelled
        );
    }

    #[test]
    fn official_sources_are_marked_in_the_prompt() {
        let (_, text) = answer("https://github.com/anthropics/skills", "c\n");
        assert!(text.contains("Official source."), "{text}");
    }

    #[test]
    fn insecure_sources_say_the_connection_is_unencrypted() {
        let (_, text) = answer("http://github.com/anthropics/skills", "a\n2\n");
        assert!(text.contains("Unencrypted connection"), "{text}");
        assert!(!text.contains("Official source."), "{text}");
        assert!(
            text.contains("[2] http://github.com/anthropics/*"),
            "{text}"
        );
        let (_, text) = answer("https://github.com/acme/skills", "c\n");
        assert!(!text.contains("Unencrypted"), "{text}");
    }
}
