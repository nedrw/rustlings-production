//! Spec for `platform/9.4-external-secrets` — references resolve strictly, and the artifact is
//! clean.

use std::collections::BTreeMap;

use platform::secrets::{Reference, SecretError, leaks, resolve};

fn reference(env: &str, path: &str, key: &str) -> Reference {
    Reference {
        env: env.to_owned(),
        path: path.to_owned(),
        key: key.to_owned(),
    }
}

fn store() -> BTreeMap<String, BTreeMap<String, String>> {
    let mut store = BTreeMap::new();
    store.insert(
        "db/mailer".to_owned(),
        BTreeMap::from([("url".to_owned(), "postgres://secret".to_owned())]),
    );
    store
}

#[test]
fn a_reference_resolves_to_its_value() {
    let values = resolve(&[reference("DATABASE_URL", "db/mailer", "url")], &store()).unwrap();
    assert_eq!(values["DATABASE_URL"], "postgres://secret");
}

#[test]
fn a_missing_key_is_an_error_naming_the_path() {
    let error = resolve(&[reference("DATABASE_URL", "db/mailer", "nope")], &store()).unwrap_err();
    assert_eq!(
        error,
        SecretError::Missing {
            path: "db/mailer".to_owned(),
            key: "nope".to_owned(),
        }
    );
}

#[test]
fn a_missing_path_is_an_error_not_an_empty_value() {
    let error = resolve(&[reference("TOKEN", "no/such", "key")], &store()).unwrap_err();
    assert_eq!(
        error,
        SecretError::Missing {
            path: "no/such".to_owned(),
            key: "key".to_owned(),
        }
    );
}

#[test]
fn an_unresolved_reference_leaves_no_partial_answer() {
    let references = [
        reference("DATABASE_URL", "db/mailer", "url"),
        reference("TOKEN", "db/mailer", "token"),
    ];
    assert!(resolve(&references, &store()).is_err());
}

#[test]
fn a_clean_manifest_leaks_nothing() {
    let manifest = vec![
        "env:".to_owned(),
        "  - name: DATABASE_URL".to_owned(),
        "    valueFrom: secretKeyRef".to_owned(),
    ];
    assert!(leaks(&manifest, &["postgres://secret".to_owned()]).is_empty());
}

#[test]
fn a_secret_value_in_the_manifest_is_a_leak_even_in_a_comment() {
    let manifest = vec![
        "env:".to_owned(),
        "  - name: DATABASE_URL".to_owned(),
        "    value: postgres://secret".to_owned(),
        "# was postgres://secret".to_owned(),
    ];
    let found = leaks(&manifest, &["postgres://secret".to_owned()]);
    assert_eq!(found.len(), 2, "{found:?}");
}
