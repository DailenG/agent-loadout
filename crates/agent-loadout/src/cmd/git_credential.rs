//! `loadout git-credential --remote <NAME> <get|store|erase>` (SPEC 9.6):
//! the credential helper git runs for a remote with `auth = token`. Git
//! writes the request on stdin; `get` answers with the remote's username
//! and the token from the OS keyring, for the remote's own protocol and
//! host only. `store` and `erase` read the request and do nothing.

use std::io::{Read, Write};

use agent_loadout_core::remote::config::RemotesFile;
use agent_loadout_core::remote::credential::{self, CredentialRequest};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::Args;

use super::Context;

#[derive(Debug, Args)]
pub struct GitCredentialArgs {
    /// The git remote whose token to answer with.
    #[arg(long, value_name = "NAME")]
    remote: String,
    /// What git asks: get, store or erase.
    action: String,
}

pub fn run(ctx: &Context, args: &GitCredentialArgs) -> Result<(), CoreError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| CoreError::new(ErrorCode::IoError, format!("cannot read stdin: {err}")))?;
    if args.action != "get" {
        return Ok(());
    }
    let remotes = RemotesFile::load(ctx.fs(), &ctx.home.state_dir())?;
    let remote = remotes.get(&args.remote).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("no remote named {:?}", args.remote),
        )
    })?;
    let request = CredentialRequest::parse(&input);
    if let Some(answer) = credential::get(remote, &request, ctx.keyring())? {
        let mut out = std::io::stdout().lock();
        out.write_all(answer.to_protocol().as_bytes())
            .and_then(|()| out.flush())
            .map_err(|err| {
                CoreError::new(ErrorCode::IoError, format!("cannot write stdout: {err}"))
            })?;
    }
    Ok(())
}
