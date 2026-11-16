//! `env:NAME`: a variable of Loadout's own environment (the injected
//! [`Env`]). Unset is `missing`; whether a client's environment has the
//! variable anyway (`unverifiable`) is a delivery question (SPEC 11.4).

use crate::env::Env;
use crate::secrets::SecretValue;
use crate::secrets::status::Resolution;

pub fn resolve(env: &Env, name: &str) -> Result<SecretValue, Resolution> {
    let Some(value) = env.var(name) else {
        return Err(Resolution::missing(format!(
            "environment variable {name} is not set"
        )));
    };
    value
        .to_str()
        .map(SecretValue::new)
        .ok_or_else(|| Resolution::error(format!("environment variable {name} is not UTF-8")))
}
