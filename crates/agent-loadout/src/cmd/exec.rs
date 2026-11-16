//! `loadout exec` in the command table (SPEC 11.5, 14).
//!
//! The command never goes through clap's parser: [`exec_args`] finds it in
//! the raw arguments, so a server's arguments reach it untouched, errors
//! keep the `loadout-exec:` prefix and the exit codes 77 and 78, and
//! nothing is ever printed to stdout. The runtime is [`crate::exec`].

use std::ffi::OsString;

use clap::Args;

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// [--ref NAME=REF]... [--op-env ID --map KEY=NAME...] -- <cmd> [args]
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARGS"
    )]
    pub args: Vec<OsString>,
}

/// The arguments after `exec` when `argv` runs `loadout exec`, with only
/// global options (`--home`, `--json`, `--yes`, which `exec` ignores)
/// before it.
pub fn exec_args(argv: &[OsString]) -> Option<&[OsString]> {
    let mut index = 1;
    while let Some(arg) = argv.get(index) {
        let arg = arg.to_str()?;
        match arg {
            "exec" => return Some(&argv[index + 1..]),
            "--json" | "--yes" => index += 1,
            "--home" => index += 2,
            _ if arg.starts_with("--home=") => index += 1,
            _ => return None,
        }
    }
    None
}
