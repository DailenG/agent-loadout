//! Reference grammar (SPEC 11.2): templates in server fields and the
//! provider references of `secrets.toml`.

use agent_loadout_core::ErrorCode;
use agent_loadout_core::secrets::grammar::{
    PathVar, SecretRef, TemplatePart, holds_reference, literal_parts, parse_template,
};
use agent_loadout_core::secrets::providers::ProviderKind;

#[test]
fn template_grammar() {
    use TemplatePart::{Env, Literal, Secret, Var};
    let ok: [(&str, Vec<TemplatePart>); 6] = [
        ("plain", vec![Literal("plain".into())]),
        ("", vec![]),
        (
            "Bearer ${secret:LINEAR_TOKEN}",
            vec![Literal("Bearer ".into()), Secret("LINEAR_TOKEN".into())],
        ),
        (
            "${home}/x:${env:PATH}${project}${loadout_home}",
            vec![
                Var(PathVar::Home),
                Literal("/x:".into()),
                Env("PATH".into()),
                Var(PathVar::Project),
                Var(PathVar::LoadoutHome),
            ],
        ),
        (
            "$${env:X} and $5 and $",
            vec![Literal("${env:X} and $5 and $".into())],
        ),
        (
            "op://not/a/ref/here",
            vec![Literal("op://not/a/ref/here".into())],
        ),
    ];
    for (text, parts) in ok {
        assert_eq!(parse_template(text).unwrap(), parts, "{text:?}");
    }
    for bad in [
        "${GITHUB_TOKEN}",
        "${env:}",
        "${env:1BAD}",
        "${secret:has-dash}",
        "${op://v/i/f}",
        "${keyring://loadout/X}",
        "tail ${env:HOME",
        "${}",
    ] {
        let err = parse_template(bad).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidRef, "{bad:?}");
    }
}

#[test]
fn secret_reference_grammar() {
    assert_eq!(
        SecretRef::parse("env:HASS_TOKEN").unwrap(),
        SecretRef::Env {
            name: "HASS_TOKEN".into()
        }
    );
    assert_eq!(
        SecretRef::parse("op://Example Vault/Fake Item/credential").unwrap(),
        SecretRef::OnePassword {
            vault: "Example Vault".into(),
            item: "Fake Item".into(),
            section: None,
            field: "credential".into(),
            query: None,
        }
    );
    assert_eq!(
        SecretRef::parse("op://v/i/s/f?attribute=otp").unwrap(),
        SecretRef::OnePassword {
            vault: "v".into(),
            item: "i".into(),
            section: Some("s".into()),
            field: "f".into(),
            query: Some("attribute=otp".into()),
        }
    );
    assert_eq!(
        SecretRef::parse("op-env://fakeenv0000/LINEAR_TOKEN").unwrap(),
        SecretRef::OnePasswordEnvironment {
            environment: "fakeenv0000".into(),
            name: "LINEAR_TOKEN".into()
        }
    );
    assert_eq!(
        SecretRef::parse("keyring://loadout/team/OPENAI_API_KEY").unwrap(),
        SecretRef::Keyring {
            service: "loadout".into(),
            account: "team/OPENAI_API_KEY".into()
        }
    );
    for bad in [
        "",
        "HASS_TOKEN",
        "env:",
        "env:BAD-NAME",
        "op://v/i",
        "op://v//f",
        "op://v/i/s/f/extra",
        "op://v/i/f?attribute",
        "op-env://UPPER/NAME",
        "op-env://env0000",
        "keyring://loadout",
        "keyring:///account",
        "${secret:X}",
        "https://example.com/secret",
    ] {
        let err = SecretRef::parse(bad).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidRef, "{bad:?}");
    }
}

/// The canonical text is what `secrets set` writes; it must parse back to
/// the same reference, and each form belongs to one provider.
#[test]
fn references_print_canonically_and_name_their_provider() {
    let cases = [
        ("env:HASS_TOKEN", ProviderKind::Env),
        (
            "op://Example Vault/Fake Item/credential",
            ProviderKind::OnePassword,
        ),
        ("op://v/i/s/f?attribute=otp", ProviderKind::OnePassword),
        (
            "op-env://fakeenv0000/LINEAR_TOKEN",
            ProviderKind::OnePassword,
        ),
        (
            "keyring://loadout/team/OPENAI_API_KEY",
            ProviderKind::Keyring,
        ),
    ];
    for (text, provider) in cases {
        let reference = SecretRef::parse(text).unwrap();
        assert_eq!(reference.to_string(), text);
        assert_eq!(SecretRef::parse(&reference.to_string()).unwrap(), reference);
        assert_eq!(reference.provider(), provider, "{text}");
    }
}

/// What the classifier looks at in a value that also holds references.
#[test]
fn literal_parts_leave_references_out() {
    assert!(holds_reference("Bearer ${secret:LINEAR_TOKEN}"));
    assert!(holds_reference("${env:HOME}/x"));
    assert!(!holds_reference("${home}/x"));
    assert!(!holds_reference("$${secret:X}"));
    assert!(!holds_reference("${broken"));
    assert_eq!(
        literal_parts("Bearer ${secret:LINEAR_TOKEN} tail"),
        ["Bearer ", " tail"]
    );
    assert_eq!(literal_parts("${env:A}${secret:B}"), Vec::<String>::new());
    assert_eq!(literal_parts("${broken"), ["${broken"]);
}
