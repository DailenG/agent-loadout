//! Template rendering for one server and client with secret delivery (SPEC
//! 11.2, 11.4): path variables become paths, and each value holding
//! references is written in the mode [`Policy::choose`] picks:
//!
//! - `env`: the client's own variable syntax (`${NAME}`, `${env:NAME}`),
//!   or a forwarded variable name (Codex);
//! - `launch`: the key leaves the entry's `env`, and the entry starts
//!   `loadout exec --ref KEY=REF ... -- <command> <args>` instead;
//! - `command`: omp and Pi get `!<loadout> secret get --ref REF`, Claude
//!   Code gets one `headersHelper` running `<loadout> secret-header`;
//! - `inline`: the resolved value, kept for masking in diffs.
//!
//! Command templates mark a value with `{secret:KEY}`, bound by `--ref
//! KEY=REF`: no `$`, `%` or `!`, so neither a client nor a shell expands it.

use std::path::Path;
use std::sync::Arc;

use crate::adapters::caps::{CommandRef, EnvRefSyntax, FieldSet};
use crate::adapters::mapper::{RemoteLaunch, StdioLaunch};
use crate::brand;
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::server::{DeliveryMode, Transport};
use crate::secrets::SecretValue;
use crate::secrets::catalog::Catalog;
use crate::secrets::delivery::{
    DeliveryEnv, Mode, Policy, Sharing, Slot, Source, check_available, field_name, op_env,
    source_of_alias,
};
use crate::secrets::grammar::{PathVar, SecretRef, TemplatePart, parse_template};
use crate::secrets::providers::{Resolver, env};
use crate::secrets::scanner::SecretMemory;

/// The placeholder opening of a bound value in command templates.
pub const PLACEHOLDER: &str = "{secret:";

/// What rendering reads: Loadout's environment (paths, inline `env:`
/// values), the catalog and this machine's providers.
#[derive(Clone, Copy)]
pub struct RenderEnv<'a> {
    pub env: &'a Env,
    pub fs: &'a dyn Fs,
    pub loadout_home: &'a Path,
    pub catalog: &'a Catalog,
    pub delivery: &'a DeliveryEnv,
    pub memory: &'a Arc<SecretMemory>,
    /// The project folder `${project}` stands for; `None` at system scope.
    pub project: Option<&'a Path>,
}

/// A stdio `env` value as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Value(String),
    /// Forwarded from the client's environment under the same name (Codex
    /// `env_vars`).
    Forward(String),
    /// Set by `loadout exec` when the server starts.
    Launched,
}

/// A header value as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    Value(String),
    /// `Authorization: Bearer <variable>` (Codex `bearer_token_env_var`).
    Bearer(String),
    /// The whole value from a variable (Codex `env_http_headers`).
    FromEnv(String),
    /// Printed by the entry's `headersHelper`.
    Helper,
}

#[derive(Debug, Clone)]
enum Piece {
    Text(String),
    Ref(Source),
}

/// Renders the values of one server for one client.
pub struct Renderer<'a> {
    rx: RenderEnv<'a>,
    policy: Policy<'a>,
    transport: Transport,
    modes: Vec<Mode>,
    launch: Vec<(String, Source)>,
    helper_refs: Vec<(String, Source)>,
    helper_headers: Vec<(String, String)>,
    /// Values written in plain text (`inline`), for masking.
    pub inline: Vec<SecretValue>,
    /// `inline_env_fallback` warnings.
    pub warnings: Vec<CoreError>,
}

impl<'a> Renderer<'a> {
    pub fn new(rx: RenderEnv<'a>, policy: Policy<'a>, transport: Transport) -> Self {
        Self {
            rx,
            policy,
            transport,
            modes: Vec::new(),
            launch: Vec::new(),
            helper_refs: Vec::new(),
            helper_headers: Vec::new(),
            inline: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// The entry's delivery: its most consequential mode, if it holds
    /// references.
    pub fn delivery(&self) -> Option<Mode> {
        self.modes.iter().copied().max()
    }

    /// A `command`, `args`, `cwd`, `url` or client field value.
    pub fn value(&mut self, text: &str, field: FieldSet) -> Result<String, CoreError> {
        let pieces = self.pieces(text)?;
        let sources = sources(&pieces);
        if sources.is_empty() {
            return self.literal(&pieces, field);
        }
        let mode = self.choose(field, &pieces, &sources, false)?;
        match mode {
            Mode::Env => self.spell(&pieces, field),
            Mode::Inline => self.inline_value(&pieces, field),
            Mode::Launch | Mode::Command => Err(CoreError::new(
                ErrorCode::DeliveryUnavailable,
                format!(
                    "`{}` cannot be delivered to `{}`",
                    mode.as_str(),
                    field_name(field)
                ),
            )),
        }
    }

    /// The value of stdio `env.<key>`.
    pub fn env_value(&mut self, key: &str, text: &str) -> Result<EnvValue, CoreError> {
        let pieces = self.pieces(text)?;
        let sources = sources(&pieces);
        if sources.is_empty() {
            return self.literal(&pieces, FieldSet::ENV).map(EnvValue::Value);
        }
        let whole = is_whole(&pieces);
        let forwardable = self.policy.forwards() && whole && sources[0].variable() == key;
        match self.choose(FieldSet::ENV, &pieces, &sources, forwardable)? {
            Mode::Env if forwardable => Ok(EnvValue::Forward(sources[0].variable().to_owned())),
            Mode::Env => self.spell(&pieces, FieldSet::ENV).map(EnvValue::Value),
            Mode::Launch => {
                let source = sources[0].clone();
                self.available(&source)?;
                self.launch.push((key.to_owned(), source));
                Ok(EnvValue::Launched)
            }
            Mode::Command => self.bang(&pieces).map(EnvValue::Value),
            Mode::Inline => self
                .inline_value(&pieces, FieldSet::ENV)
                .map(EnvValue::Value),
        }
    }

    /// The value of header `name`.
    pub fn header(&mut self, name: &str, text: &str) -> Result<HeaderValue, CoreError> {
        let pieces = self.pieces(text)?;
        let sources = sources(&pieces);
        if sources.is_empty() {
            return self
                .literal(&pieces, FieldSet::HEADERS)
                .map(HeaderValue::Value);
        }
        let whole = is_whole(&pieces);
        let bearer = name.eq_ignore_ascii_case("authorization")
            && matches!(pieces.as_slice(), [Piece::Text(prefix), Piece::Ref(_)] if prefix == "Bearer ");
        let forwardable = self.policy.forwards() && (whole || bearer);
        match self.choose(FieldSet::HEADERS, &pieces, &sources, forwardable)? {
            Mode::Env if forwardable => {
                let variable = sources[0].variable().to_owned();
                Ok(if bearer {
                    HeaderValue::Bearer(variable)
                } else {
                    HeaderValue::FromEnv(variable)
                })
            }
            Mode::Env => self
                .spell(&pieces, FieldSet::HEADERS)
                .map(HeaderValue::Value),
            Mode::Command if self.policy.caps.command_ref == Some(CommandRef::HeadersHelper) => {
                let template = self.template(&pieces, true)?;
                self.helper_headers.push((name.to_owned(), template));
                Ok(HeaderValue::Helper)
            }
            Mode::Command => self.bang(&pieces).map(HeaderValue::Value),
            Mode::Inline => self
                .inline_value(&pieces, FieldSet::HEADERS)
                .map(HeaderValue::Value),
            Mode::Launch => Err(CoreError::new(
                ErrorCode::DeliveryUnavailable,
                "`launch` delivery cannot bring a header value",
            )),
        }
    }

    /// Wraps the command in `loadout exec` when a value is launched.
    pub fn finish_stdio(&mut self, stdio: &mut StdioLaunch) -> Result<(), CoreError> {
        if self.launch.is_empty() {
            return Ok(());
        }
        let launcher = self.rx.delivery.launcher(self.policy.sharing)?;
        let mut args = vec!["exec".to_owned()];
        let mut environment: Option<String> = None;
        let mut maps = Vec::new();
        if let Some(account) = self.op_account(self.launch.iter().map(|(_, s)| s)) {
            args.push("--op-account".to_owned());
            args.push(account.to_owned());
        }
        for (key, source) in &self.launch {
            match source.reference() {
                SecretRef::OnePasswordEnvironment {
                    environment: id,
                    name,
                } => {
                    if environment.as_ref().is_some_and(|other| *other != id) {
                        return Err(CoreError::new(
                            ErrorCode::DeliveryUnavailable,
                            "`loadout exec` takes variables from one 1Password Environment per \
                             server; this server uses several",
                        ));
                    }
                    environment = Some(id);
                    maps.push(format!("{key}={name}"));
                }
                reference => {
                    args.push("--ref".to_owned());
                    args.push(format!("{key}={reference}"));
                }
            }
        }
        if let Some(id) = environment {
            args.push("--op-env".to_owned());
            args.push(id);
            for map in maps {
                args.push("--map".to_owned());
                args.push(map);
            }
        }
        args.push("--".to_owned());
        args.push(std::mem::take(&mut stdio.command));
        args.append(&mut stdio.args);
        stdio.command = launcher;
        stdio.args = args;
        Ok(())
    }

    /// Sets the `headersHelper` of the entry when a header uses it.
    pub fn finish_remote(&mut self, remote: &mut RemoteLaunch) -> Result<(), CoreError> {
        if self.helper_headers.is_empty() {
            return Ok(());
        }
        let os = self.rx.env.os();
        let mut command = vec![
            shell_quote(os, &self.rx.delivery.launcher(self.policy.sharing)?)?,
            "secret-header".to_owned(),
        ];
        if let Some(account) = self.op_account(self.helper_refs.iter().map(|(_, s)| s)) {
            command.push("--op-account".to_owned());
            command.push(shell_quote(os, account)?);
        }
        for (key, source) in &self.helper_refs {
            command.push("--ref".to_owned());
            command.push(shell_quote(os, &format!("{key}={}", source.reference()))?);
        }
        for (name, template) in &self.helper_headers {
            command.push(shell_quote(os, &format!("{name}={template}"))?);
        }
        remote.headers_helper = Some(command.join(" "));
        Ok(())
    }

    fn choose(
        &mut self,
        field: FieldSet,
        pieces: &[Piece],
        sources: &[Source],
        forwardable: bool,
    ) -> Result<Mode, CoreError> {
        let mode = self.policy.choose(&Slot {
            field,
            transport: self.transport,
            whole: is_whole(pieces),
            forwardable,
            sources,
        })?;
        if mode == Mode::Inline && self.policy.requested == DeliveryMode::Auto {
            let names: Vec<&str> = sources.iter().map(Source::variable).collect();
            self.warnings.push(CoreError::new(
                ErrorCode::InlineEnvFallback,
                format!(
                    "{} cannot get `${{env:{}}}` in `{}` otherwise; the value from {}'s \
                     environment is written into its config file",
                    self.policy.client,
                    names.join("}`, `${env:"),
                    field_name(field),
                    brand::PRODUCT_NAME
                ),
            ));
        }
        self.modes.push(mode);
        Ok(mode)
    }

    /// The template as pieces: path variables filled in, references looked
    /// up in the catalog.
    fn pieces(&self, text: &str) -> Result<Vec<Piece>, CoreError> {
        let mut pieces: Vec<Piece> = Vec::new();
        let push_text = |pieces: &mut Vec<Piece>, text: &str| match pieces.last_mut() {
            Some(Piece::Text(last)) => last.push_str(text),
            _ => pieces.push(Piece::Text(text.to_owned())),
        };
        for part in parse_template(text)? {
            match part {
                TemplatePart::Literal(literal) => push_text(&mut pieces, &literal),
                TemplatePart::Var(PathVar::Home) => {
                    push_text(&mut pieces, &self.rx.env.home().to_string_lossy());
                }
                TemplatePart::Var(PathVar::LoadoutHome) => {
                    push_text(&mut pieces, &self.rx.loadout_home.to_string_lossy());
                }
                TemplatePart::Var(PathVar::Project) => match self.rx.project {
                    Some(project) => push_text(&mut pieces, &project.to_string_lossy()),
                    None => {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRef,
                            format!("`${{project}}` in {text:?} has no value at system scope"),
                        ));
                    }
                },
                TemplatePart::Env(name) => pieces.push(Piece::Ref(Source::Env { name })),
                TemplatePart::Secret(alias) => {
                    pieces.push(Piece::Ref(source_of_alias(self.rx.catalog, &alias)?));
                }
            }
        }
        Ok(pieces)
    }

    /// Literal text; refused where the client would expand a `${` in it.
    fn literal(&self, pieces: &[Piece], field: FieldSet) -> Result<String, CoreError> {
        let mut out = String::new();
        for piece in pieces {
            if let Piece::Text(text) = piece {
                self.check_literal(text, field)?;
                out.push_str(text);
            }
        }
        Ok(out)
    }

    fn check_literal(&self, text: &str, field: FieldSet) -> Result<(), CoreError> {
        if self.policy.env_syntax(field).is_some() && text.contains("${") {
            return Err(CoreError::new(
                ErrorCode::DeliveryUnavailable,
                format!(
                    "{} would expand the literal `${{` in {text:?}; it has no escape for it",
                    self.policy.client
                ),
            ));
        }
        Ok(())
    }

    /// `env` delivery in the client's syntax.
    fn spell(&self, pieces: &[Piece], field: FieldSet) -> Result<String, CoreError> {
        let syntax = self.policy.env_syntax(field);
        let mut out = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    self.check_literal(text, field)?;
                    out.push_str(text);
                }
                Piece::Ref(source) => {
                    let name = source.variable();
                    match syntax {
                        Some(EnvRefSyntax::DollarBrace { .. }) => {
                            out.push_str(&format!("${{{name}}}"))
                        }
                        Some(EnvRefSyntax::DollarBraceEnv) => {
                            out.push_str(&format!("${{env:{name}}}"));
                        }
                        Some(EnvRefSyntax::BraceEnv) => out.push_str(&format!("{{env:{name}}}")),
                        Some(EnvRefSyntax::CodexForward) | None => {
                            return Err(CoreError::new(
                                ErrorCode::DeliveryUnavailable,
                                format!(
                                    "{} does not expand variables in `{}`",
                                    self.policy.client,
                                    field_name(field)
                                ),
                            ));
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// `inline` delivery: values resolved now.
    fn inline_value(&mut self, pieces: &[Piece], field: FieldSet) -> Result<String, CoreError> {
        let mut out = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    self.check_literal(text, field)?;
                    out.push_str(text);
                }
                Piece::Ref(source) => {
                    let value = self.resolve(source)?;
                    out.push_str(value.expose());
                    self.inline.push(value);
                }
            }
        }
        Ok(out)
    }

    fn resolve(&self, source: &Source) -> Result<SecretValue, CoreError> {
        let value = match source {
            Source::Env { name } => {
                env::resolve(self.rx.env, name).map_err(|resolution| resolution.into_error(name))?
            }
            Source::Provider {
                alias,
                reference: SecretRef::OnePasswordEnvironment { .. },
                ..
            } => {
                return Err(CoreError::new(
                    ErrorCode::DeliveryUnavailable,
                    format!(
                        "{alias}: 1Password Environment variables reach a server only through \
                         `op run` when it starts (`launch` delivery); they are never written inline"
                    ),
                ));
            }
            Source::Provider {
                alias, reference, ..
            } => {
                let keyring = self.rx.delivery.keyring();
                let env = op_env(self.rx.env, keyring, self.rx.catalog.op_account.as_deref());
                let op = self.rx.delivery.op(self.rx.env, self.rx.fs);
                Resolver::new(&env, keyring, op, self.rx.memory.clone())
                    .resolve(reference)
                    .map_err(|resolution| resolution.into_error(alias))?
            }
        };
        self.rx.memory.record(value.expose());
        Ok(value)
    }

    fn available(&self, source: &Source) -> Result<(), CoreError> {
        check_available(
            source,
            self.rx.env,
            self.rx.fs,
            self.rx.delivery,
            self.rx.memory,
        )
    }

    /// `--op-account` for a `loadout` command resolving `sources`: the
    /// configured 1Password account when any of them is a 1Password
    /// reference (gate V1 FB-7). Never in a shared file: the account is
    /// this machine's, so a committed file relies on `OP_ACCOUNT` or each
    /// machine's own setting, which `loadout exec` reads at run time.
    fn op_account<'s>(&self, mut sources: impl Iterator<Item = &'s Source>) -> Option<&'a str> {
        if self.policy.sharing == Sharing::Shared {
            return None;
        }
        let account = self.rx.catalog.op_account.as_deref()?;
        sources
            .any(|source| {
                matches!(
                    source.reference(),
                    SecretRef::OnePassword { .. } | SecretRef::OnePasswordEnvironment { .. }
                )
            })
            .then_some(account)
    }

    /// `!<loadout> secret get --ref REF` (omp, Pi), with a template when the
    /// value has text around the reference.
    fn bang(&mut self, pieces: &[Piece]) -> Result<String, CoreError> {
        let os = self.rx.env.os();
        let launcher = shell_quote(os, &self.rx.delivery.launcher(self.policy.sharing)?)?;
        let mut command = format!("!{launcher} secret get");
        if let [Piece::Ref(source)] = pieces {
            self.available(source)?;
            if let Some(account) = self.op_account(std::iter::once(source)) {
                command.push_str(&format!(" --op-account {}", shell_quote(os, account)?));
            }
            command.push_str(&format!(
                " --ref {}",
                shell_quote(os, &source.reference().to_string())?
            ));
            return Ok(command);
        }
        let start = self.helper_refs.len();
        let template = self.template(pieces, false)?;
        let bound: Vec<(String, Source)> = self.helper_refs.drain(start..).collect();
        if let Some(account) = self.op_account(bound.iter().map(|(_, s)| s)) {
            command.push_str(&format!(" --op-account {}", shell_quote(os, account)?));
        }
        for (key, source) in &bound {
            command.push_str(&format!(
                " --ref {}",
                shell_quote(os, &format!("{key}={}", source.reference()))?
            ));
        }
        command.push_str(&format!(" --template {}", shell_quote(os, &template)?));
        Ok(command)
    }

    /// The pieces as a command template; references are bound in
    /// `helper_refs` (shared by every header of a `headersHelper` when
    /// `shared`).
    fn template(&mut self, pieces: &[Piece], shared: bool) -> Result<String, CoreError> {
        let start = if shared { 0 } else { self.helper_refs.len() };
        let mut out = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    if text.contains(PLACEHOLDER) {
                        return Err(CoreError::new(
                            ErrorCode::DeliveryUnavailable,
                            format!(
                                "the literal {PLACEHOLDER:?} in {text:?} cannot be written in a command template"
                            ),
                        ));
                    }
                    out.push_str(text);
                }
                Piece::Ref(source) => {
                    self.available(source)?;
                    let key = source.label().to_owned();
                    let bound = self.helper_refs[start..].iter().find(|(k, _)| *k == key);
                    match bound {
                        Some((_, existing)) if existing != source => {
                            return Err(CoreError::new(
                                ErrorCode::DeliveryUnavailable,
                                format!("{key} names two different references here"),
                            ));
                        }
                        Some(_) => {}
                        None => self.helper_refs.push((key.clone(), source.clone())),
                    }
                    out.push_str(&format!("{PLACEHOLDER}{key}}}"));
                }
            }
        }
        Ok(out)
    }
}

fn sources(pieces: &[Piece]) -> Vec<Source> {
    pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::Ref(source) => Some(source.clone()),
            Piece::Text(_) => None,
        })
        .collect()
}

fn is_whole(pieces: &[Piece]) -> bool {
    matches!(pieces, [Piece::Ref(_)])
}

/// One argument of a command line run through the client's shell: single
/// quotes on Unix; double quotes on Windows. Which shell a Windows client
/// uses (`cmd.exe`, or a POSIX shell such as Git Bash) is not settled
/// (`scripts/verify-clients.md` DX-6, DX-7), so only text both read the
/// same inside double quotes passes: `"`, `%`, `$`, a backtick, a line
/// break and a trailing backslash are refused.
pub fn shell_quote(os: Os, text: &str) -> Result<String, CoreError> {
    match os {
        Os::Linux | Os::MacOs => Ok(format!("'{}'", text.replace('\'', r"'\''"))),
        Os::Windows => {
            if text.contains(['"', '%', '$', '`', '\n', '\r']) || text.ends_with('\\') {
                return Err(CoreError::new(
                    ErrorCode::DeliveryUnavailable,
                    format!(
                        "{text:?} cannot be quoted for a Windows command line (`\"`, `%`, `$`, a \
                         backtick, a line break or a trailing backslash)"
                    ),
                ));
            }
            Ok(format!("\"{text}\""))
        }
    }
}

/// Renders a command template: each `{secret:KEY}` becomes `values[KEY]`.
/// Fails on a placeholder with no value or an unterminated one.
pub fn fill_template(
    template: &str,
    values: &[(String, SecretValue)],
) -> Result<SecretValue, CoreError> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find(PLACEHOLDER) {
        out.push_str(&rest[..start]);
        let after = &rest[start + PLACEHOLDER.len()..];
        let end = after.find('}').ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRef,
                format!("unterminated {PLACEHOLDER:?} in the template"),
            )
        })?;
        let key = &after[..end];
        let value = values
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidRef,
                    format!(
                        "the template uses {PLACEHOLDER}{key}}} but no --ref {key}=REF is given"
                    ),
                )
            })?;
        out.push_str(value.expose());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(SecretValue::new(out))
}

/// `KEY=REF` as given to `loadout exec`, `secret get` and `secret-header`:
/// the key is a variable name, the reference a provider reference.
pub fn parse_binding(text: &str) -> Result<(String, SecretRef), CoreError> {
    let invalid = || {
        CoreError::new(
            ErrorCode::InvalidRef,
            format!("expected NAME=REF (a variable name and a provider reference), got {text:?}"),
        )
    };
    let (key, reference) = text.split_once('=').ok_or_else(invalid)?;
    if !crate::model::ids::is_valid(crate::model::ids::IdKind::EnvVar, key) {
        return Err(invalid());
    }
    Ok((key.to_owned(), SecretRef::parse(reference)?))
}
