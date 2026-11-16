#![cfg(feature = "sim")]
//! M3a Task 9: station information (CHT-06) and local macros (CHT-05). A macro
//! is expanded into text for the draft; expansion never sends anything.
//! History search is bounded and parameterised.

use mc_core::app::{
    expand, spawn_app, AppCommand, AppError, AppHandle, AppOptions, CommandOk, MacroContext, MacroDef, MAX_TEXT_BYTES,
};
use mc_core::profile::StationProfile;
use mc_storage::Store;
use std::path::Path;

fn ctx() -> MacroContext {
    MacroContext {
        call: Some("ZS1XYZ".into()),
        mycall: "ZS6ABC".into(),
        name: Some("Shaun".into()),
        qth: Some("Pretoria".into()),
        loc: Some("KG33xi".into()),
        rig: Some("IC-7300".into()),
        pwr: Some("50 W".into()),
        ant: Some("EFHW".into()),
        about: Some("Digital modes fan".into()),
        email: Some("me@example.org".into()),
        utc: "07:45".into(),
        allow_email: false,
    }
}

#[test]
fn expands_known_tags_from_profile_and_peer() {
    let out = expand("<CALL> de <MYCALL>: <NAME>, <QTH> <LOC>, <RIG> <PWR> <ANT>. <ABOUT> at <UTC>Z", &ctx()).unwrap();
    assert_eq!(out.text, "ZS1XYZ de ZS6ABC: Shaun, Pretoria KG33xi, IC-7300 50 W EFHW. Digital modes fan at 07:45Z");
    assert!(out.missing.is_empty());
}

#[test]
fn missing_value_stays_visible_and_is_reported() {
    let mut c = ctx();
    c.rig = None;
    c.call = None;
    let out = expand("Hi <CALL>, rig <RIG>, again <RIG>, <NAME>", &c).unwrap();
    assert_eq!(out.text, "Hi <CALL>, rig <RIG>, again <RIG>, Shaun");
    assert_eq!(out.missing, ["CALL", "RIG"], "each missing tag once, in order of appearance");
}

#[test]
fn email_needs_explicit_permission() {
    let mut c = ctx();
    let out = expand("Mail <EMAIL>", &c).unwrap();
    assert_eq!(out.text, "Mail <EMAIL>", "never inserted without the operator's explicit action");
    c.allow_email = true;
    assert_eq!(expand("Mail <EMAIL>", &c).unwrap().text, "Mail me@example.org");
    c.email = None;
    let out = expand("Mail <EMAIL>", &c).unwrap();
    assert_eq!((out.text.as_str(), out.missing.as_slice()), ("Mail <EMAIL>", ["EMAIL".to_string()].as_slice()));
}

#[test]
fn expansion_over_4096_bytes_is_an_error() {
    assert_eq!(MAX_TEXT_BYTES, 4096);
    let mut c = ctx();
    c.about = Some("x".repeat(128));
    // 32 x 128 = 4096 bytes exactly: fits.
    let fits = expand(&"<ABOUT>".repeat(32), &c).unwrap();
    assert_eq!(fits.text.len(), 4096);
    // One more tag: over the cap, refused, never cut.
    let errs = expand(&"<ABOUT>".repeat(33), &c).unwrap_err();
    assert_eq!(errs[0].field, "text");
    assert!(errs[0].message.contains("4096"), "{}", errs[0].message);
}

#[test]
fn unknown_tag_untouched() {
    let out = expand("a <FOO> <call> <> <NAME b> < NAME> <NAME", &ctx()).unwrap();
    assert_eq!(out.text, "a <FOO> <call> <> <NAME b> < NAME> <NAME");
    assert!(out.missing.is_empty());
    // A value is never scanned again: a tag typed into a field stays literal.
    let mut c = ctx();
    c.name = Some("<QTH>".into());
    assert_eq!(expand("<NAME>", &c).unwrap().text, "<QTH>");
}

// --- through the service ---------------------------------------------------

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

fn sim_profile() -> StationProfile {
    let mut p = StationProfile::new_simulator("ZS6ABC");
    p.name = Some("Shaun".into());
    p.grid = Some("KG33xi".into());
    p.qth = Some("Pretoria".into());
    p.rig = Some("IC-7300".into());
    p.power = Some("50 W".into());
    p.antenna = Some("EFHW".into());
    p.email = Some("me@example.org".into());
    p.about = Some("Digital modes".into());
    p
}

/// A started app with `profile` saved and selected; `before` runs on the
/// station's store before the app opens it.
async fn start(root: &Path, before: impl FnOnce(&mut Store)) -> (AppHandle, String) {
    let h = spawn_app(Store::open(&root.join("app.db")).unwrap(), opts(root));
    let id = match h.call(AppCommand::SaveProfile { profile: sim_profile() }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let dir = root.join("stations").join(&id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Store::open(&dir.join("station.db")).unwrap();
    before(&mut s);
    drop(s);
    h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
    (h, id)
}

async fn macros(h: &AppHandle) -> Vec<MacroDef> {
    match h.call(AppCommand::Macros).await.unwrap() {
        CommandOk::Macros { macros } => macros,
        other => panic!("{other:?}"),
    }
}

async fn expand_macro(
    h: &AppHandle,
    id: &str,
    peer: Option<&str>,
    allow_email: bool,
) -> Result<(String, Vec<String>), AppError> {
    match h.call(AppCommand::ExpandMacro { id: id.into(), peer: peer.map(String::from), allow_email }).await? {
        CommandOk::Expanded { text, missing } => Ok((text, missing)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn defaults_seeded_once() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = start(dir.path(), |_| {}).await;
    let first = macros(&h).await;
    let names: Vec<&str> = first.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Station info", "Greeting", "Closing"]);
    assert!(first.iter().all(|m| !m.id.is_empty() && m.body.len() <= MAX_TEXT_BYTES));
    assert_eq!(macros(&h).await, first, "a second call seeds nothing more");

    // Delete them all: they are not seeded again.
    for m in &first {
        h.call(AppCommand::DeleteMacro { id: m.id.clone() }).await.unwrap();
    }
    assert!(macros(&h).await.is_empty(), "the operator deleted them on purpose");

    // Add, edit, reorder-stable listing.
    let saved = match h
        .call(AppCommand::SaveMacro {
            macro_: Box::new(MacroDef { id: String::new(), name: " QRZ? ".into(), body: "QRZ <CALL>".into() }),
        })
        .await
        .unwrap()
    {
        CommandOk::Macro(m) => m,
        other => panic!("{other:?}"),
    };
    assert_eq!((saved.name.as_str(), saved.id.is_empty()), ("QRZ?", false));
    let edited = MacroDef { body: "QRZ <CALL> de <MYCALL>".into(), ..saved.clone() };
    h.call(AppCommand::SaveMacro { macro_: Box::new(edited.clone()) }).await.unwrap();
    assert_eq!(macros(&h).await, [edited]);
    // An unknown id is NotFound.
    assert!(matches!(h.call(AppCommand::DeleteMacro { id: "nope".into() }).await, Err(AppError::NotFound { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn macro_validation_is_operator_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = start(dir.path(), |_| {}).await;
    for (name, body, field) in [
        ("", "x", "name"),
        ("n\u{7}", "x", "name"),
        (&"n".repeat(33), "x", "name"),
        ("ok", "  ", "body"),
        ("ok", &"b".repeat(MAX_TEXT_BYTES + 1), "body"),
        ("ok", "bell\u{7}", "body"),
    ] {
        let r = h
            .call(AppCommand::SaveMacro {
                macro_: Box::new(MacroDef { id: String::new(), name: name.into(), body: body.into() }),
            })
            .await;
        match r {
            Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, field, "{name:?}"),
            other => panic!("{name:?}: {other:?}"),
        }
    }
    // Newlines in a body are fine.
    h.call(AppCommand::SaveMacro {
        macro_: Box::new(MacroDef { id: String::new(), name: "ok".into(), body: "a\nb".into() }),
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn expand_does_not_queue_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = start(dir.path(), |_| {}).await;
    let station = macros(&h).await.into_iter().find(|m| m.name == "Station info").unwrap();
    let (text, missing) = expand_macro(&h, &station.id, Some("zs1xyz"), false).await.unwrap();
    assert!(text.contains("Shaun") && text.contains("Pretoria") && text.contains("KG33xi"), "{text}");
    assert!(missing.is_empty(), "{missing:?}");
    let greeting = macros(&h).await.into_iter().find(|m| m.name == "Greeting").unwrap();
    let (text, missing) = expand_macro(&h, &greeting.id, None, false).await.unwrap();
    assert!(text.contains("<CALL>") && text.contains("ZS6ABC"), "{text}");
    assert_eq!(missing, ["CALL"]);
    assert!(matches!(
        h.call(AppCommand::ExpandMacro {
            id: greeting.id.clone(),
            peer: Some("not a call".into()),
            allow_email: false
        })
        .await,
        Err(AppError::Validation { .. })
    ));
    assert!(matches!(expand_macro(&h, "nope", None, false).await, Err(AppError::NotFound { .. })));

    // Nothing was sent, queued or drafted.
    match h.call(AppCommand::Conversations).await.unwrap() {
        CommandOk::Conversations { conversations } => assert!(conversations.is_empty(), "{conversations:?}"),
        other => panic!("{other:?}"),
    }
    match h.call(AppCommand::History { peer: "ZS1XYZ".into(), before_seq: None, limit: 50 }).await.unwrap() {
        CommandOk::History { messages } => assert!(messages.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn search_history_by_text_and_callsign() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = start(dir.path(), |s| {
        s.store_incoming_chat("ZS1XYZ", &[1; 16], "Signal report 59 on 20m", 1).unwrap();
        s.store_incoming_chat("ZS1XYZ", &[2; 16], "100% sure, snake_case", 1).unwrap();
        s.store_incoming_chat("K1ABC", &[3; 16], "Good morning", 1).unwrap();
        s.insert_outgoing("K1ABC", &[4; 16], "Thanks for the report").unwrap();
        for i in 0u8..120 {
            s.store_incoming_chat("W1AW", &[10, i, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], &format!("bulk {i}"), 1)
                .unwrap();
        }
    })
    .await;
    let search = |q: &str, limit: u32| {
        let h = h.clone();
        let q = q.to_string();
        async move { h.call(AppCommand::SearchHistory { query: q, limit }).await }
    };
    let bodies = |r: Result<CommandOk, AppError>| match r.unwrap() {
        CommandOk::History { messages } => messages.into_iter().map(|m| m.body).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    // By text, either direction, case-insensitive, newest first.
    assert_eq!(bodies(search("REPORT", 50).await), ["Thanks for the report", "Signal report 59 on 20m"]);
    // By callsign.
    assert_eq!(bodies(search("k1abc", 50).await), ["Thanks for the report", "Good morning"]);
    // LIKE wildcards are plain characters.
    assert_eq!(bodies(search("%", 50).await), ["100% sure, snake_case"]);
    assert_eq!(bodies(search("_", 50).await), ["100% sure, snake_case"]);
    assert_eq!(bodies(search("n_k", 50).await), Vec::<String>::new());
    assert_eq!(bodies(search("x' OR '1'='1", 50).await), Vec::<String>::new());
    // The limit is clamped to 1..=100.
    assert_eq!(bodies(search("bulk", 1000).await).len(), 100);
    assert_eq!(bodies(search("bulk", 0).await).len(), 1);
    assert_eq!(bodies(search("bulk", 7).await).len(), 7);
    // An empty or over-long query is refused, readably.
    for q in ["", "   ", &"q".repeat(129)] {
        match search(q, 10).await {
            Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, "query"),
            other => panic!("{q:?}: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_station_info_validation() {
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
    // Saved and read back, trimmed; blank means none.
    let mut p = sim_profile();
    p.qth = Some("  Pretoria  ".into());
    p.rig = Some("   ".into());
    let saved = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p,
        other => panic!("{other:?}"),
    };
    assert_eq!((saved.qth.as_deref(), saved.rig.as_deref()), (Some("Pretoria"), None));
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => assert_eq!(s.profiles, std::slice::from_ref(&saved)),
        other => panic!("{other:?}"),
    }
    // Every field is bounded at 128 characters and refuses control characters.
    type Set = fn(&mut StationProfile, String);
    let fields: [(&str, Set); 6] = [
        ("qth", |p, v| p.qth = Some(v)),
        ("rig", |p, v| p.rig = Some(v)),
        ("power", |p, v| p.power = Some(v)),
        ("antenna", |p, v| p.antenna = Some(v)),
        ("email", |p, v| p.email = Some(v)),
        ("about", |p, v| p.about = Some(v)),
    ];
    for (name, set) in fields {
        let mut p = sim_profile();
        set(&mut p, "é".repeat(128));
        p.validate().unwrap_or_else(|e| panic!("{name}: 128 characters fit: {e:?}"));
        set(&mut p, "é".repeat(129));
        let errs = p.validate().unwrap_err();
        assert_eq!((errs[0].field.as_str(), errs[0].message.as_str()), (name, "At most 128 characters"));
        for bad in ["a\nb", "a\u{0}b", "a\u{7f}b", "a\tb"] {
            set(&mut p, bad.into());
            let errs = p.validate().unwrap_err();
            assert_eq!((errs[0].field.as_str(), errs[0].message.as_str()), (name, "No control characters"), "{bad:?}");
        }
    }
}
