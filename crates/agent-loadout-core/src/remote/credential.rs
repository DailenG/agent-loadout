//! `loadout git-credential` (SPEC 9.6, `auth = token`): git's credential
//! helper protocol for one git remote. `get` answers with the remote's
//! `username` (default `x-access-token`) and the token stored in the OS
//! keyring for that remote, and only for the protocol and host of the
//! remote's own URL; `store` and `erase` do nothing (git never writes the
//! keyring through Loadout).

use std::collections::BTreeMap;

use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::remote::config::RemoteConfig;
use crate::remote::git::{GitAuth, url_scheme_host};
use crate::secrets::SecretValue;
use crate::secrets::providers::keyring::KeyringBackend;

/// The username sent when the remote sets none; GitHub accepts it with any
/// token (other hosts may need the account name [verify per host]).
pub const DEFAULT_USERNAME: &str = "x-access-token";

/// The keyring account (service [`brand::KEYRING_SERVICE`]) holding a git
/// remote's token.
pub fn token_account(remote: &str) -> String {
    format!("git-remote/{remote}")
}

/// What git asks for: the attributes it sent (`protocol`, `host`, `path`,
/// `username`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CredentialRequest {
    pub attributes: BTreeMap<String, String>,
}

impl CredentialRequest {
    /// `key=value` lines up to the first empty line or the end.
    pub fn parse(input: &str) -> Self {
        let attributes = input
            .lines()
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Self { attributes }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).map(String::as_str)
    }
}

/// The answer to a `get`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: SecretValue,
}

impl Credential {
    /// The protocol's output: `username=...` and `password=...` lines.
    pub fn to_protocol(&self) -> String {
        format!(
            "username={}\npassword={}\n",
            self.username,
            self.password.expose()
        )
    }
}

/// Answers `get` for `remote`. `Ok(None)` when the request is for another
/// protocol or host than the remote's URL (git then tries elsewhere);
/// `missing_secret` when no token is stored.
pub fn get(
    remote: &RemoteConfig,
    request: &CredentialRequest,
    keyring: &dyn KeyringBackend,
) -> Result<Option<Credential>, CoreError> {
    if GitAuth::parse(remote.auth.as_deref())? != GitAuth::Token {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("remote {} does not use auth = token", remote.name),
        ));
    }
    let url = remote.url.as_deref().unwrap_or_default();
    let Some((scheme, host)) = url_scheme_host(url) else {
        return Ok(None);
    };
    let asked_scheme = request.get("protocol").unwrap_or_default();
    let asked_host = request.get("host").unwrap_or_default();
    if !asked_scheme.eq_ignore_ascii_case(&scheme) || !asked_host.eq_ignore_ascii_case(&host) {
        return Ok(None);
    }
    let account = token_account(&remote.name);
    let token = keyring
        .get(brand::KEYRING_SERVICE, &account)
        .map_err(|resolution| {
            CoreError::new(
                ErrorCode::SecretError,
                format!(
                    "cannot read the token of remote {}: {}",
                    remote.name,
                    resolution.message.unwrap_or_default()
                ),
            )
        })?
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::MissingSecret,
                format!(
                    "no token stored for remote {}; pipe one into `{} remote token {}`",
                    remote.name,
                    brand::BIN_NAME,
                    remote.name
                ),
            )
        })?;
    Ok(Some(Credential {
        username: remote
            .username
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_USERNAME.to_owned()),
        password: token,
    }))
}
