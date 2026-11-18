//! M3b Task 7: Mercury's TUNE command and its query answer.

use mercury_adapter::control::{parse_event, ControlEvent};
use mercury_adapter::{Command, TuneArg};

#[test]
fn tune_wire_forms() {
    assert_eq!(Command::Tune(TuneArg::Level(-20.0)).wire(), "TUNE -20\r");
    assert_eq!(Command::Tune(TuneArg::Level(-12.5)).wire(), "TUNE -12.5\r");
    assert_eq!(Command::Tune(TuneArg::Level(0.0)).wire(), "TUNE 0\r");
    assert_eq!(Command::Tune(TuneArg::Query).wire(), "TUNE ?\r");
    assert_eq!(Command::Tune(TuneArg::Off).wire(), "TUNE OFF\r");
    assert_eq!(TuneArg::RANGE, (-60.0, 0.0));
}

#[test]
fn the_query_answer_parses() {
    assert_eq!(parse_event("TUNE -20"), ControlEvent::Tune("-20".into()));
    assert_eq!(parse_event("TUNE -12.5"), ControlEvent::Tune("-12.5".into()));
    assert!(matches!(parse_event("TUNE"), ControlEvent::Unknown(_)));
}
