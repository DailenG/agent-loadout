//! Source keys: which repository a git URL or local path names, and the
//! owner and host keys above it.
//!
//! A git URL is parsed the way git and curl parse it (WHATWG, the `url`
//! crate), after refusing anything that could make them contact another
//! host or path than the key names: `?`, `#`, `\`, `.`/`..` segments,
//! percent-encoded `.`, `/` or `\`, empty segments, control characters, a
//! second `@`. Git is handed the URL rebuilt from the parsed parts
//! ([`SourceRef::fetch_url`]), so what is trusted is what git contacts.
//! Sources reached over plain `http://` or `git://` have keys of their own
//! (`http://host/path`): trust given over HTTPS or SSH does not admit them.

use std::path::{Path, PathBuf};

use url::Url;

use crate::error::{CoreError, ErrorCode};
use crate::model::sources::{SourceScope, key_shape};

/// Hosts that serve anyone's repositories: trusting the whole host would
/// trust every publisher on it, so only an owner or a repository can be
/// trusted there.
pub const AGGREGATOR_HOSTS: [&str; 7] = [
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "gitee.com",
    "git.sr.ht",
    "huggingface.co",
];

/// Hosts whose owner and repository names are case-insensitive.
const CASE_INSENSITIVE_HOSTS: [&str; 4] =
    ["github.com", "gitlab.com", "bitbucket.org", "codeberg.org"];

/// Owners shown with an "official" marker (`host/owner`). The marker never
/// skips the prompt.
pub const OFFICIAL_OWNERS: [&str; 2] = ["github.com/anthropics", "github.com/modelcontextprotocol"];

/// How git reaches a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Transport {
    Https,
    Http,
    Ssh,
    Git,
    /// `user@host:path`.
    Scp,
    Local,
}

impl Transport {
    fn of_scheme(scheme: &str) -> Option<Self> {
        match scheme.to_ascii_lowercase().as_str() {
            "https" => Some(Transport::Https),
            "http" => Some(Transport::Http),
            "ssh" => Some(Transport::Ssh),
            "git" => Some(Transport::Git),
            _ => None,
        }
    }

    const fn scheme(self) -> &'static str {
        match self {
            Transport::Https => "https",
            Transport::Http => "http",
            Transport::Ssh | Transport::Scp => "ssh",
            Transport::Git => "git",
            Transport::Local => "file",
        }
    }

    /// Left out of keys and URLs, so `https://` and `ssh://` URLs of one
    /// repository give one key.
    const fn default_port(self) -> Option<u16> {
        match self {
            Transport::Https => Some(443),
            Transport::Http => Some(80),
            Transport::Ssh => Some(22),
            Transport::Git => Some(9418),
            Transport::Scp | Transport::Local => None,
        }
    }

    /// The key prefix of a transport without encryption.
    const fn marker(self) -> &'static str {
        match self {
            Transport::Http => "http://",
            Transport::Git => "git://",
            _ => "",
        }
    }
}

/// A source: the repository a git URL or a local path names. Never holds
/// credentials ([`SourceRef::fetch_url`] takes them from the URL given).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceRef {
    /// `github.com/acme/skills`, `http://git.acme.corp/tools` or
    /// `file:///C:/repos/x`.
    key: String,
    /// The canonical URL without credentials (a local source: its
    /// `file:///` key).
    url: String,
    /// Remote sources: what follows `scheme://[userinfo@]` in the URL
    /// (`host[:port]/path`), or `host:path` for `user@host:path`.
    address: String,
    /// Remote sources: the host (with a non-default port).
    host: Option<String>,
    /// Remote sources whose path has two or more segments: the first.
    owner: Option<String>,
    transport: Transport,
}

/// A remote URL in canonical parts. The userinfo is only carried to
/// [`SourceRef::fetch_url`].
struct Remote {
    transport: Transport,
    /// `user[:password]` as given (percent-encoded).
    userinfo: Option<String>,
    /// Lowercase, without a trailing dot, with a non-default `:port`.
    host: String,
    /// `user@host:/path` (rather than a path below the user's home).
    absolute: bool,
    /// Checked and percent-normalized, before any cut.
    segments: Vec<String>,
}

enum Parsed {
    Remote(Remote),
    /// An absolute path (`/`-separated or not).
    Local(String),
}

fn invalid(text: &str, why: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidValue,
        format!(
            "{:?} is not a source: {why}",
            SourceRef::redact_userinfo(text)
        ),
    )
}

/// `user@host:path` (not a Windows drive path): `(user@host, path)`.
fn scp_like(text: &str) -> Option<(&str, &str)> {
    let (user_host, rest) = text.split_once(':')?;
    if !user_host.contains('@') || user_host.contains(['/', '\\']) || rest.starts_with("//") {
        return None;
    }
    Some((user_host, rest))
}

fn is_windows_drive(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_local_path(text: &str) -> bool {
    Path::new(text).is_absolute() || is_windows_drive(text) || text.starts_with('/')
}

/// What could make git contact another host or path than the key names.
fn check_text(text: &str) -> Result<(), &'static str> {
    if text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("it contains spaces or control characters");
    }
    if text.contains('\\') {
        return Err("it contains a backslash");
    }
    if text.contains(['?', '#']) {
        return Err("a query or fragment (`?`, `#`) does not name a repository");
    }
    let lower = text.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c"]
        .iter()
        .any(|code| lower.contains(code))
    {
        return Err("it percent-encodes `.`, `/` or `\\`");
    }
    if text.matches('@').count() > 1 {
        return Err("it has more than one `@`");
    }
    Ok(())
}

/// The segments of a path (without its leading `/`): no empty, `.` or `..`
/// segment; one trailing `/` is fine.
fn checked_segments(path: &str) -> Result<Vec<&str>, &'static str> {
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty() {
        return Ok(Vec::new());
    }
    path.split('/')
        .map(|segment| match segment {
            "" => Err("it has an empty path segment (`//`)"),
            "." | ".." => Err("it has a `.` or `..` path segment"),
            segment => Ok(segment),
        })
        .collect()
}

/// Percent escapes of unreserved characters decoded, the others in
/// uppercase hex, so one name has one spelling.
fn normalize_escapes(segment: &str) -> Result<String, &'static str> {
    let mut out = String::with_capacity(segment.len());
    let mut rest = segment;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let hex = rest
            .get(at + 1..at + 3)
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("it has a `%` that is not a percent-encoded byte")?;
        let byte = u8::from_str_radix(hex, 16).map_err(|_| "it has a bad percent escape")?;
        if byte.is_ascii_alphanumeric() || b"-_~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push_str(&hex.to_ascii_uppercase());
        }
        rest = &rest[at + 3..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Lowercase, without a trailing dot (`github.com.` is `github.com`); a
/// name of non-empty labels or a bracketed IPv6 address.
fn canonical_host(host: &str) -> Result<String, &'static str> {
    let lower = host.to_ascii_lowercase();
    let host = lower.strip_suffix('.').unwrap_or(&lower);
    let ok = match host.strip_prefix('[').and_then(|ip| ip.strip_suffix(']')) {
        Some(ip) => {
            !ip.is_empty()
                && ip
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        }
        None => {
            !host.is_empty()
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b)
                        })
                })
        }
    };
    if ok {
        Ok(host.to_owned())
    } else {
        Err("its host is not a host name")
    }
}

/// `scheme://[userinfo@]host[:port]/path`.
fn url_remote(text: &str, transport: Transport) -> Result<Remote, &'static str> {
    check_text(text)?;
    let rest = text.split_once("://").map_or("", |(_, rest)| rest);
    let (authority, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .is_empty()
    {
        return Err("it has no host");
    }
    if raw_path.contains('@') {
        return Err("it has an `@` in the path");
    }
    let raw = checked_segments(raw_path)?;
    let parsed = Url::parse(text).map_err(|_| "it is not a valid URL")?;
    let host = canonical_host(parsed.host_str().ok_or("it has no host")?)?;
    let host = match parsed.port() {
        Some(port) if Some(port) != transport.default_port() => format!("{host}:{port}"),
        _ => host,
    };
    let userinfo = match (parsed.username(), parsed.password()) {
        ("", None) => None,
        (user, None) => Some(user.to_owned()),
        (user, Some(password)) => Some(format!("{user}:{password}")),
    };
    let path = parsed.path();
    let path = path.strip_prefix('/').unwrap_or(path);
    let segments = checked_segments(path)?
        .into_iter()
        .map(normalize_escapes)
        .collect::<Result<Vec<_>, _>>()?;
    // Dot segments were refused above, so the parser cannot have dropped
    // any; a difference means the text is not what it seems.
    if segments.len() != raw.len() {
        return Err("it is not a valid URL");
    }
    Ok(Remote {
        transport,
        userinfo,
        host,
        absolute: true,
        segments,
    })
}

/// `user@host:path`: git takes the path as written (no percent escapes).
fn scp_remote(text: &str, user_host: &str, path: &str) -> Result<Remote, &'static str> {
    check_text(text)?;
    if path.contains(['%', '@']) {
        return Err("an scp-style path cannot contain `%` or `@`");
    }
    let (user, host) = user_host.rsplit_once('@').unwrap_or(("", user_host));
    let host = canonical_host(host)?;
    let absolute = path.starts_with('/');
    let segments = checked_segments(path.strip_prefix('/').unwrap_or(path))?
        .into_iter()
        .map(str::to_owned)
        .collect();
    Ok(Remote {
        transport: Transport::Scp,
        userinfo: (!user.is_empty()).then(|| user.to_owned()),
        host,
        absolute,
        segments,
    })
}

impl SourceRef {
    /// The source a git URL names: `https://`, `http://`, `ssh://`,
    /// `git://`, `user@host:path`, `file://` or an absolute local path (a
    /// local repository). `invalid_value` for anything else, and for a URL
    /// git could read as another host or path than its key names.
    pub fn from_git_url(url: &str) -> Result<Self, CoreError> {
        let text = url.trim();
        Self::parse(text, text).map(|(source, _)| source)
    }

    /// The repository a source key (`host/path`, `http://host/path`,
    /// `file:///path`) or a git URL names.
    pub fn from_key_or_url(text: &str) -> Result<Self, CoreError> {
        let text = text.trim();
        Self::parse(&as_url(text), text).map(|(source, _)| source)
    }

    /// The canonical form of a source key or git URL, with the scope its
    /// shape stands for: `host/path` (or a URL) a repository,
    /// `host/owner/*` an owner, `host/*` a host; keys of plain `http://`
    /// and `git://` sources keep that prefix. `sources.toml` keys and the
    /// CLI's key arguments go through it (`github.com/Acme/*` is
    /// `github.com/acme/*`, `github.com./x/y.git` is `github.com/x/y`).
    pub fn canonical_key(text: &str) -> Result<(String, SourceScope), CoreError> {
        let text = text.trim();
        let Some(base) = text.strip_suffix("/*") else {
            return Ok((Self::from_key_or_url(text)?.key, SourceScope::Repo));
        };
        // `base/x` names a repository below the owner or host.
        let Parsed::Remote(remote) = Self::split(&as_url(&format!("{base}/x")), text)? else {
            return Err(invalid(text, "a local source has no owner or host key"));
        };
        match remote.segments.as_slice() {
            [_] => Ok((
                format!("{}{}/*", remote.transport.marker(), remote.host),
                SourceScope::Host,
            )),
            [_, _] => {
                let (source, _) = Self::from_remote(remote).map_err(|why| invalid(text, why))?;
                match source.scope_key(SourceScope::Owner) {
                    Some(key) => Ok((key, SourceScope::Owner)),
                    None => Err(invalid(text, "it has no owner")),
                }
            }
            _ => Err(invalid(
                text,
                "a key ending in `/*` is host/* (a host) or host/owner/* (an owner)",
            )),
        }
    }

    /// `text` with the userinfo of every URL in it replaced by `***`, for
    /// messages: a URL can carry a token (`https://user:token@host/...`).
    pub fn redact_userinfo(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find("://") {
            let (head, tail) = rest.split_at(at + 3);
            out.push_str(head);
            let end = tail
                .find(|c: char| c == '/' || c.is_whitespace() || matches!(c, '"' | '\'' | '`'))
                .unwrap_or(tail.len());
            match tail[..end].rfind('@') {
                Some(at) => {
                    out.push_str("***");
                    out.push_str(&tail[at..end]);
                }
                None => out.push_str(&tail[..end]),
            }
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    }

    /// `text` split into a remote URL's parts or a local path; `shown` in
    /// errors.
    fn split(text: &str, shown: &str) -> Result<Parsed, CoreError> {
        if text.to_ascii_lowercase().starts_with("file://") {
            let path = &text["file://".len()..];
            // `file:///C:/x` and `file:///home/x`; a `file://host/...` share
            // keeps its host as the first path part.
            return Ok(Parsed::Local(match path.strip_prefix('/') {
                Some(rest) if is_windows_drive(rest) => rest.to_owned(),
                Some(_) => path.to_owned(),
                None => format!("//{path}"),
            }));
        }
        let remote = if let Some((scheme, _)) = text.split_once("://") {
            let transport = Transport::of_scheme(scheme)
                .ok_or_else(|| invalid(shown, "unsupported URL scheme"))?;
            url_remote(text, transport)
        } else if let Some((user_host, path)) = scp_like(text) {
            scp_remote(text, user_host, path)
        } else if is_local_path(text) {
            return Ok(Parsed::Local(text.to_owned()));
        } else {
            return Err(invalid(shown, "not a git URL or an absolute path"));
        };
        remote
            .map(Parsed::Remote)
            .map_err(|why| invalid(shown, why))
    }

    /// The source and the userinfo given with it.
    fn parse(text: &str, shown: &str) -> Result<(Self, Option<String>), CoreError> {
        match Self::split(text, shown)? {
            Parsed::Local(path) => Ok((Self::local(&path, shown)?, None)),
            Parsed::Remote(remote) => Self::from_remote(remote).map_err(|why| invalid(shown, why)),
        }
    }

    /// A local folder or repository (absolute path).
    pub fn from_local_path(path: &Path) -> Result<Self, CoreError> {
        let text = path.to_string_lossy();
        if !is_local_path(&text) {
            return Err(invalid(&text, "not an absolute path"));
        }
        Self::local(&text, &text)
    }

    fn local(path: &str, original: &str) -> Result<Self, CoreError> {
        let mut path = path.replace('\\', "/");
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        if is_windows_drive(&path) {
            path[..1].make_ascii_uppercase();
        }
        let rest = if path.starts_with("//") {
            path.as_str()
        } else {
            path.trim_start_matches('/')
        };
        let key = format!("file:///{rest}");
        if key_shape(&key) != Some(SourceScope::Repo) {
            return Err(invalid(original, "not a usable local path"));
        }
        Ok(Self {
            url: key.clone(),
            key,
            address: String::new(),
            host: None,
            owner: None,
            transport: Transport::Local,
        })
    }

    /// The repository of `remote`: the path up to GitLab's `/-/`, only
    /// `owner/repo` on GitHub, without a trailing `.git`, lowercase on
    /// case-insensitive hosts.
    fn from_remote(remote: Remote) -> Result<(Self, Option<String>), &'static str> {
        let Remote {
            transport,
            userinfo,
            host,
            absolute,
            mut segments,
        } = remote;
        if let Some(cut) = segments.iter().position(|part| part == "-") {
            segments.truncate(cut);
        }
        if host == "github.com" {
            segments.truncate(2);
        }
        if CASE_INSENSITIVE_HOSTS.contains(&host.as_str()) {
            segments
                .iter_mut()
                .for_each(|part| part.make_ascii_lowercase());
        }
        let dot_git = match segments.last_mut() {
            Some(last) if last.ends_with(".git") => {
                last.truncate(last.len() - ".git".len());
                if last.is_empty() {
                    return Err("it has no repository name");
                }
                true
            }
            _ => false,
        };
        if segments.is_empty() {
            return Err("it has no repository path");
        }
        let path = segments.join("/");
        let key = format!("{}{host}/{path}", transport.marker());
        if key_shape(&key) != Some(SourceScope::Repo) {
            return Err("it has characters a source key cannot hold");
        }
        let fetch_path = if dot_git { format!("{path}.git") } else { path };
        let address = if transport == Transport::Scp {
            format!("{host}:{}{fetch_path}", if absolute { "/" } else { "" })
        } else {
            format!("{host}/{fetch_path}")
        };
        // An SSH user is part of the address, not a secret: it is shown.
        let user = match transport {
            Transport::Ssh | Transport::Scp => userinfo
                .as_deref()
                .map(|info| info.split_once(':').map_or(info, |(user, _)| user)),
            _ => None,
        };
        let url = with_userinfo(transport, user, &address);
        let source = Self {
            key,
            url,
            address,
            owner: (segments.len() >= 2).then(|| segments[0].clone()),
            host: Some(host),
            transport,
        };
        Ok((source, userinfo))
    }

    /// The repository key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The canonical URL without credentials (an SSH user stays); a local
    /// source's `file:///` key.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The URL git must fetch the source from: the canonical URL rebuilt
    /// from the parsed parts, with the userinfo of `given` (the URL as the
    /// user gave it) when `given` names this same source. Never show it or
    /// store it: it can carry a token. A local source: `given` when it
    /// names this source, else its `file:///` URL.
    pub fn fetch_url(&self, given: &str) -> String {
        let given = given.trim();
        let Ok((other, userinfo)) = Self::parse(given, "") else {
            return self.url.clone();
        };
        let same = other.transport == self.transport
            && other.key == self.key
            && other.address == self.address;
        match (same, self.transport, userinfo) {
            (true, Transport::Local, _) => given.to_owned(),
            (true, _, Some(userinfo)) => {
                with_userinfo(self.transport, Some(&userinfo), &self.address)
            }
            _ => self.url.clone(),
        }
    }

    /// Fetched over a connection without encryption (`http://`, `git://`):
    /// anyone on the network path can change what arrives.
    pub fn is_insecure(&self) -> bool {
        !self.transport.marker().is_empty()
    }

    pub fn is_local(&self) -> bool {
        self.host.is_none()
    }

    /// A local source's path, for showing it under its placeholder root.
    pub fn local_path(&self) -> Option<PathBuf> {
        if !self.is_local() {
            return None;
        }
        let rest = self.key.strip_prefix("file:///")?;
        Some(if is_windows_drive(rest) || rest.starts_with("//") {
            PathBuf::from(rest)
        } else {
            PathBuf::from(format!("/{rest}"))
        })
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// The key of `scope` (`host/owner/*`, `host/*`, with the `http://` or
    /// `git://` prefix of an insecure source); `None` when the source has
    /// no such scope (a local source, a repository without an owner
    /// segment).
    pub fn scope_key(&self, scope: SourceScope) -> Option<String> {
        let marker = self.transport.marker();
        match scope {
            SourceScope::Repo => Some(self.key.clone()),
            SourceScope::Owner => Some(format!(
                "{marker}{}/{}/*",
                self.host.as_ref()?,
                self.owner.as_ref()?
            )),
            SourceScope::Host => Some(format!("{marker}{}/*", self.host.as_ref()?)),
        }
    }

    /// Every key whose trust applies, most specific first.
    pub fn matching_keys(&self) -> Vec<String> {
        [SourceScope::Repo, SourceScope::Owner, SourceScope::Host]
            .into_iter()
            .filter_map(|scope| self.scope_key(scope))
            .collect()
    }

    /// Every key whose block applies, most specific first: the
    /// [`Self::matching_keys`], then for an insecure source the keys of the
    /// same source over an encrypted transport (a block covers every
    /// transport, a trust only its own).
    pub fn block_keys(&self) -> Vec<String> {
        let mut keys = self.matching_keys();
        let marker = self.transport.marker();
        if !marker.is_empty() {
            let secure: Vec<String> = keys
                .iter()
                .filter_map(|key| key.strip_prefix(marker).map(str::to_owned))
                .collect();
            keys.extend(secure);
        }
        keys
    }

    /// The scopes "always trust" offers, narrowest first: a host only
    /// when it is not an [`AGGREGATOR_HOSTS`] host.
    pub fn scopes(&self) -> Vec<SourceScope> {
        let mut scopes = vec![SourceScope::Repo];
        if self.owner.is_some() {
            scopes.push(SourceScope::Owner);
        }
        if let Some(host) = &self.host
            && !AGGREGATOR_HOSTS.contains(&host.as_str())
        {
            scopes.push(SourceScope::Host);
        }
        scopes
    }

    /// An [`OFFICIAL_OWNERS`] repository, fetched over an encrypted
    /// transport.
    pub fn is_official(&self) -> bool {
        match (&self.host, &self.owner) {
            (Some(host), Some(owner)) if !self.is_insecure() => {
                OFFICIAL_OWNERS.contains(&format!("{host}/{owner}").as_str())
            }
            _ => false,
        }
    }
}

/// `scheme://[userinfo@]address`, or `[userinfo@]address` for
/// `user@host:path`.
fn with_userinfo(transport: Transport, userinfo: Option<&str>, address: &str) -> String {
    let at = userinfo.map(|info| format!("{info}@")).unwrap_or_default();
    match transport {
        Transport::Scp => format!("{at}{address}"),
        _ => format!("{}://{at}{address}", transport.scheme()),
    }
}

/// A source key without a scheme (`host/path`) as an `https://` URL; URLs,
/// `user@host:path` and local paths as they are.
fn as_url(text: &str) -> String {
    if text.contains("://") || scp_like(text).is_some() || is_local_path(text) {
        text.to_owned()
    } else {
        format!("https://{text}")
    }
}
