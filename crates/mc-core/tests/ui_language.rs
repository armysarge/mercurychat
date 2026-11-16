//! M3a Task 9b: the interface language is a core setting (`ui.language`): one
//! of system | en | es | pt-BR | fr, validated, stored, and kept across a
//! restart. The UI reads it from the snapshot at start-up.

use mc_core::app::{spawn_app, AppCommand, AppError, AppHandle, AppOptions, CommandOk};
use mc_storage::Store;
use std::path::Path;
use std::time::Duration;

fn start(db: &Path) -> AppHandle {
    spawn_app(Store::open(db).unwrap(), AppOptions::default())
}

async fn language(h: &AppHandle) -> String {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s.ui_language,
        other => panic!("{other:?}"),
    }
}

async fn set(h: &AppHandle, language: &str) -> Result<CommandOk, AppError> {
    h.call(AppCommand::SetUiLanguage { language: language.into() }).await
}

#[tokio::test(flavor = "multi_thread")]
async fn language_defaults_to_system() {
    let dir = tempfile::tempdir().unwrap();
    let h = start(&dir.path().join("mercurychat.db"));
    assert_eq!(language(&h).await, "system");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_supported_languages_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let h = start(&dir.path().join("mercurychat.db"));
    for ok in ["system", "en", "es", "pt-BR", "fr"] {
        set(&h, ok).await.unwrap_or_else(|e| panic!("{ok}: {e:?}"));
        assert_eq!(language(&h).await, ok);
    }
    for bad in ["", "de", "EN", "pt", "pt-br", "fr ", "fr-FR", "../fr"] {
        match set(&h, bad).await {
            Err(AppError::Validation { errors }) => {
                assert_eq!(errors.len(), 1, "{bad:?}");
                assert_eq!(errors[0].field, "language");
                assert!(errors[0].message.contains("pt-BR"), "the message names the choices: {}", errors[0].message);
            }
            other => panic!("{bad:?} should be refused, got {other:?}"),
        }
        assert_eq!(language(&h).await, "fr", "a refused value changes nothing");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn language_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    {
        let h = start(&db);
        set(&h, "pt-BR").await.unwrap();
        assert_eq!(language(&h).await, "pt-BR");
    }
    tokio::time::sleep(Duration::from_millis(200)).await; // the first service has stopped
    let h = start(&db);
    assert_eq!(language(&h).await, "pt-BR");
}
