//! The logbook as ADIF or CSV, and one conversation as plain text (M5 §7).
//! Pure functions over the stored rows; the service writes the file where
//! the operator chose.
//!
//! ADIF's mode (design §10): ADIF has no Mercury mode. A record carries
//! `MODE=DYNAMIC` with no SUBMODE (DYNAMIC is ADIF 3.1's mode for
//! adaptive-rate ARQ modems; its listed submodes are VARA's, which would be
//! false here) and the application field `APP_MERCURYCHAT_MODE=MERCURY`, which
//! loggers that do not know it keep or skip without an error.

use super::logbook::Qso;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportFormat {
    Adif,
    Csv,
}

/// One line of a conversation as exported: a message or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRow {
    Message { utc: String, outgoing: bool, body: String, state: String },
    File { utc: String, outgoing: bool, name: String, size: u64, state: String },
}

impl ChatRow {
    fn utc(&self) -> &str {
        match self {
            ChatRow::Message { utc, .. } | ChatRow::File { utc, .. } => utc,
        }
    }
}

/// A stored UTC time (`2026-11-23T10:15:30.123Z`) as ADIF's date and time
/// (`20261123`, `101530`) and as text (`2026-11-23 10:15:30`).
struct Utc {
    date: String,
    time: String,
    text: String,
}

fn utc(s: &str) -> Option<Utc> {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r.clone()).filter(|d| d.iter().all(u8::is_ascii_digit)).map(|_| &s[r]);
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, se) =
        (digits(0..4)?, digits(5..7)?, digits(8..10)?, digits(11..13)?, digits(14..16)?, digits(17..19)?);
    Some(Utc { date: format!("{y}{mo}{d}"), time: format!("{h}{mi}{se}"), text: format!("{y}-{mo}-{d} {h}:{mi}:{se}") })
}

/// ASCII for ADIF's String fields: accented Latin letters lose their accents,
/// anything else non-ASCII (and any control character) becomes `?`, and line
/// breaks become spaces (String fields are one line).
fn ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let plain = match c {
            '\r' | '\n' | '\t' => " ",
            ' '..='~' => {
                out.push(c);
                continue;
            }
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => "a",
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ą' => "A",
            'ç' | 'ć' | 'č' => "c",
            'Ç' | 'Ć' | 'Č' => "C",
            'ď' => "d",
            'Ď' => "D",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
            'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę' | 'Ě' => "E",
            'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
            'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => "I",
            'ł' => "l",
            'Ł' => "L",
            'ñ' | 'ń' | 'ň' => "n",
            'Ñ' | 'Ń' | 'Ň' => "N",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
            'ř' => "r",
            'Ř' => "R",
            'ś' | 'š' | 'ş' => "s",
            'Ś' | 'Š' | 'Ş' => "S",
            'ť' | 'ţ' => "t",
            'Ť' | 'Ţ' => "T",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' => "U",
            'ý' | 'ÿ' => "y",
            'Ý' | 'Ÿ' => "Y",
            'ź' | 'ż' | 'ž' => "z",
            'Ź' | 'Ż' | 'Ž' => "Z",
            'ß' => "ss",
            'æ' => "ae",
            'Æ' => "AE",
            'œ' => "oe",
            'Œ' => "OE",
            _ => "?",
        };
        out.push_str(plain);
    }
    out
}

/// `<NAME:len>value ` for a value with something in it; nothing otherwise.
fn field(out: &mut String, name: &str, value: Option<&str>) {
    let Some(v) = value.map(ascii) else { return };
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    if v.is_empty() {
        return;
    }
    out.push_str(&format!("<{name}:{}>{v} ", v.len()));
}

/// Watts as ADIF's TX_PWR (a number): the leading number of `50 W` or `100`.
fn watts(power: &str) -> Option<String> {
    let t = power.trim();
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let n = &t[..end];
    n.parse::<f64>().ok().filter(|w| w.is_finite() && *w > 0.0).map(|_| n.to_string())
}

fn freq_mhz(q: &Qso) -> Option<String> {
    q.freq.as_ref().map(|r| format!("{:.6}", r.value as f64 / 1e6))
}

/// ADIF 3.1.4: a header, then one record per QSO.
pub fn adif(qsos: &[Qso], program_version: &str) -> String {
    let mut out = String::from("MercuryChat logbook export\n");
    field(&mut out, "ADIF_VER", Some("3.1.4"));
    field(&mut out, "PROGRAMID", Some("MercuryChat"));
    field(&mut out, "PROGRAMVERSION", Some(program_version));
    out.push_str("<EOH>\n");
    for q in qsos {
        let mut r = String::new();
        let on = utc(&q.start_utc);
        let off = q.end_utc.as_deref().and_then(utc);
        field(&mut r, "CALL", Some(&q.peer));
        field(&mut r, "QSO_DATE", on.as_ref().map(|u| u.date.as_str()));
        field(&mut r, "TIME_ON", on.as_ref().map(|u| u.time.as_str()));
        field(&mut r, "QSO_DATE_OFF", off.as_ref().map(|u| u.date.as_str()));
        field(&mut r, "TIME_OFF", off.as_ref().map(|u| u.time.as_str()));
        field(&mut r, "FREQ", freq_mhz(q).as_deref());
        field(&mut r, "BAND", q.band.as_deref());
        field(&mut r, "MODE", Some("DYNAMIC"));
        field(&mut r, "APP_MERCURYCHAT_MODE", Some("MERCURY"));
        field(&mut r, "RST_SENT", q.rst_sent.as_deref());
        field(&mut r, "RST_RCVD", q.rst_rcvd.as_deref());
        field(&mut r, "NAME", q.name.as_deref());
        field(&mut r, "QTH", q.qth.as_deref());
        field(&mut r, "GRIDSQUARE", q.grid.as_deref());
        field(&mut r, "TX_PWR", q.my_power.as_deref().and_then(watts).as_deref());
        field(&mut r, "COMMENT", q.notes.as_deref());
        out.push_str(r.trim_end());
        out.push_str(" <EOR>\n");
    }
    out
}

/// RFC 4180: quoted when it holds a comma, a quote or a line break.
fn cell(v: &str) -> String {
    if v.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// CSV of the same fields as ADIF, UTF-8 with a BOM and a header row.
pub fn csv(qsos: &[Qso]) -> String {
    const HEADER: &str = "call,qso_date,time_on,qso_date_off,time_off,freq_mhz,band,mode,rst_sent,rst_rcvd,name,qth,gridsquare,tx_power,comment,state";
    let mut out = format!("\u{feff}{HEADER}\r\n");
    for q in qsos {
        let on = utc(&q.start_utc);
        let off = q.end_utc.as_deref().and_then(utc);
        let date = |u: &Option<Utc>| u.as_ref().map(|u| u.text[..10].to_string()).unwrap_or_default();
        let time = |u: &Option<Utc>| u.as_ref().map(|u| u.text[11..].to_string()).unwrap_or_default();
        let o = |v: &Option<String>| v.clone().unwrap_or_default();
        let row = [
            q.peer.clone(),
            date(&on),
            time(&on),
            date(&off),
            time(&off),
            freq_mhz(q).unwrap_or_default(),
            o(&q.band),
            "MERCURY".into(),
            o(&q.rst_sent),
            o(&q.rst_rcvd),
            o(&q.name),
            o(&q.qth),
            o(&q.grid),
            o(&q.my_power),
            o(&q.notes),
            q.state.clone(),
        ];
        out.push_str(&row.iter().map(|v| cell(v)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1000 => format!("{b} B"),
        b if b < 1_000_000 => format!("{:.1} kB", b as f64 / 1e3),
        b => format!("{:.1} MB", b as f64 / 1e6),
    }
}

/// One conversation as plain UTF-8 text, in time order, times in UTC. Files
/// are listed by name and size; their contents are never included.
pub fn conversation_text(peer: &str, rows: &[ChatRow]) -> String {
    const YOU: &str = "You";
    let who_width = peer.chars().count().max(YOU.len());
    let mut rows: Vec<&ChatRow> = rows.iter().collect();
    rows.sort_by(|a, b| a.utc().cmp(b.utc()));
    let mut out = format!("MercuryChat conversation with {peer}\nTimes are UTC.\n\n");
    for r in rows {
        let when = utc(r.utc()).map(|u| u.text).unwrap_or_else(|| r.utc().to_string());
        let (outgoing, text) = match r {
            ChatRow::Message { outgoing, body, state, .. } => {
                let note = match state.as_str() {
                    "failed" | "uncertain" | "queued" => format!("  [{state}]"),
                    _ => String::new(),
                };
                (*outgoing, format!("{body}{note}"))
            }
            ChatRow::File { outgoing, name, size, state, .. } => {
                let dir = if *outgoing { "file sent" } else { "file received" };
                (*outgoing, format!("[{dir}] {name} ({}, {state})", size_text(*size)))
            }
        };
        let who = if outgoing { YOU } else { peer };
        let lead = format!("{when}  {who:<who_width$}  ");
        let indent = " ".repeat(lead.chars().count());
        for (i, line) in text.lines().enumerate() {
            out.push_str(if i == 0 { &lead } else { &indent });
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The records an export holds: all, or only the finalised ones.
pub fn select(qsos: Vec<Qso>, finalised_only: bool) -> Vec<Qso> {
    qsos.into_iter().filter(|q| !finalised_only || q.state == "final").collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Reading;

    fn qso() -> Qso {
        Qso {
            id: "q1".into(),
            session_id: Some("s1".into()),
            peer: "ZS6ABC".into(),
            start_utc: "2026-11-23T10:15:30.123Z".into(),
            end_utc: Some("2026-11-23T10:42:05.000Z".into()),
            grid: Some("KG33xi".into()),
            band: Some("20m".into()),
            freq: Some(Reading {
                value: 14_105_000,
                source: "operator".into(),
                at_utc: "2026-11-23T10:15:30.123Z".into(),
                age_ms: None,
            }),
            bandwidth: Some("2300".into()),
            tx_modes: Some("4,5".into()),
            rx_modes: Some("4".into()),
            rst_sent: Some("-5".into()),
            rst_rcvd: Some("+2".into()),
            name: Some("Alex".into()),
            qth: Some("Pretoria".into()),
            my_power: Some("50 W".into()),
            notes: Some("First MercuryChat QSO\non 20m".into()),
            state: "final".into(),
            rst_rcvd_source: None,
            name_source: None,
            qth_source: None,
        }
    }

    /// Every `<NAME:len>value` in `text`, with the length checked against the
    /// bytes that follow.
    fn fields(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(i) = rest.find('<') {
            rest = &rest[i + 1..];
            let close = rest.find('>').unwrap();
            let tag = &rest[..close];
            rest = &rest[close + 1..];
            let mut parts = tag.split(':');
            let name = parts.next().unwrap().to_string();
            if let Some(len) = parts.next() {
                let len: usize = len.parse().unwrap();
                assert!(rest.is_char_boundary(len), "{name}: length {len} cuts a character");
                out.push((name, rest[..len].to_string()));
                rest = &rest[len..];
            } else {
                out.push((name, String::new()));
            }
        }
        out
    }

    fn get<'a>(f: &'a [(String, String)], name: &str) -> Option<&'a str> {
        f.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn adif_header_and_eoh() {
        let text = adif(&[], "0.5.0");
        assert!(!text.starts_with('<'), "a header starts with text, not a field");
        assert!(text.contains("<ADIF_VER:5>3.1.4"), "{text}");
        assert!(text.contains("<PROGRAMID:11>MercuryChat"), "{text}");
        assert!(text.contains("<PROGRAMVERSION:5>0.5.0"), "{text}");
        assert!(text.contains("<EOH>"), "{text}");
        assert!(!text.contains("<EOR>"));
    }

    #[test]
    fn adif_record_fields() {
        let text = adif(&[qso()], "0.5.0");
        let body = &text[text.find("<EOH>").unwrap()..];
        let f = fields(body);
        assert_eq!(get(&f, "CALL"), Some("ZS6ABC"));
        assert_eq!(get(&f, "QSO_DATE"), Some("20261123"));
        assert_eq!(get(&f, "TIME_ON"), Some("101530"));
        assert_eq!(get(&f, "QSO_DATE_OFF"), Some("20261123"));
        assert_eq!(get(&f, "TIME_OFF"), Some("104205"));
        assert_eq!(get(&f, "FREQ"), Some("14.105000"));
        assert_eq!(get(&f, "BAND"), Some("20m"));
        assert_eq!(get(&f, "RST_SENT"), Some("-5"));
        assert_eq!(get(&f, "RST_RCVD"), Some("+2"));
        assert_eq!(get(&f, "NAME"), Some("Alex"));
        assert_eq!(get(&f, "QTH"), Some("Pretoria"));
        assert_eq!(get(&f, "GRIDSQUARE"), Some("KG33xi"));
        assert_eq!(get(&f, "TX_PWR"), Some("50"));
        assert_eq!(get(&f, "COMMENT"), Some("First MercuryChat QSO on 20m"), "one line");
        assert_eq!(f.last().map(|(n, _)| n.as_str()), Some("EOR"));
    }

    #[test]
    fn adif_lengths_are_ascii_and_correct() {
        let mut q = qso();
        q.name = Some("José Müller".into());
        q.qth = Some("São Paulo 東京".into());
        q.notes = Some("73 – ok".into());
        let text = adif(&[q], "0.5.0");
        assert!(text.is_ascii(), "{text}");
        let f = fields(&text);
        assert_eq!(get(&f, "NAME"), Some("Jose Muller"));
        assert_eq!(get(&f, "QTH"), Some("Sao Paulo ??"));
        assert_eq!(get(&f, "COMMENT"), Some("73 ? ok"));
    }

    #[test]
    fn adif_skips_empty_fields() {
        let mut q = qso();
        q.end_utc = None;
        q.grid = None;
        q.name = Some("  ".into());
        q.freq = None;
        q.my_power = Some("about a hundred".into());
        q.notes = None;
        let text = adif(&[q], "0.5.0");
        for absent in ["TIME_OFF", "QSO_DATE_OFF", "GRIDSQUARE", "NAME", "FREQ", "TX_PWR", "COMMENT"] {
            assert!(!text.contains(&format!("<{absent}:")), "{absent} written: {text}");
        }
        assert!(text.contains("<CALL:6>ZS6ABC"));
    }

    #[test]
    fn finalised_only_filters() {
        let mut draft = qso();
        draft.id = "q2".into();
        draft.state = "draft".into();
        let all = vec![qso(), draft];
        assert_eq!(select(all.clone(), false).len(), 2);
        let finals = select(all, true);
        assert_eq!(finals.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["q1"]);
    }

    #[test]
    fn csv_quotes_commas_and_quotes() {
        let mut q = qso();
        q.name = Some("Smith, \"Doc\"".into());
        q.qth = Some("São Paulo".into());
        let text = csv(&[q]);
        let mut lines = text.trim_start_matches('\u{feff}').split("\r\n");
        let header = lines.next().unwrap();
        assert!(header.starts_with("call,qso_date,time_on,"), "{header}");
        let row = lines.next().unwrap();
        assert!(row.starts_with("ZS6ABC,2026-11-23,10:15:30,"), "{row}");
        assert!(row.contains(",\"Smith, \"\"Doc\"\"\","), "{row}");
        assert!(row.contains(",São Paulo,"), "UTF-8 kept in CSV: {row}");
        assert!(row.contains(",\"First MercuryChat QSO\non 20m\","), "a line break is quoted: {row}");
        assert!(text.starts_with('\u{feff}'), "a BOM, so spreadsheets read UTF-8");
    }

    #[test]
    fn conversation_text_has_utc_times_and_lists_files() {
        let rows = vec![
            ChatRow::File {
                utc: "2026-11-23T10:17:00.000Z".into(),
                outgoing: true,
                name: "antenna.jpg".into(),
                size: 12_345,
                state: "complete".into(),
            },
            ChatRow::Message {
                utc: "2026-11-23T10:15:30.123Z".into(),
                outgoing: false,
                body: "hello\nfrom Pretoria".into(),
                state: "received".into(),
            },
            ChatRow::Message {
                utc: "2026-11-23T10:16:02.000Z".into(),
                outgoing: true,
                body: "hi there".into(),
                state: "uncertain".into(),
            },
        ];
        let text = conversation_text("ZS6ABC", &rows);
        assert!(text.starts_with("MercuryChat conversation with ZS6ABC"), "{text}");
        assert!(text.contains("Times are UTC"), "{text}");
        let hello = text.find("2026-11-23 10:15:30  ZS6ABC  hello").expect(&text);
        let hi = text.find("2026-11-23 10:16:02  You     hi there  [uncertain]").expect(&text);
        let file = text.find("2026-11-23 10:17:00  You     [file sent] antenna.jpg (12.3 kB, complete)").expect(&text);
        assert!(hello < hi && hi < file, "in time order: {text}");
        assert!(
            text.contains("\n                             from Pretoria\n"),
            "a continuation line is indented: {text}"
        );
    }

    /// Three records as the owner imports them into Log4OM (Task 10 Step 3;
    /// that import is M5's exit check): a full one, one with accented name
    /// and QTH on 40 m, and a bare one (no end, no frequency).
    fn sample() -> Vec<Qso> {
        let full = qso();
        let mut accents = qso();
        accents.id = "q2".into();
        accents.peer = "PY2XYZ".into();
        accents.start_utc = "2026-11-24T18:02:11.000Z".into();
        accents.end_utc = Some("2026-11-24T18:20:59.000Z".into());
        accents.band = Some("40m".into());
        accents.freq = Some(Reading {
            value: 7_104_000,
            source: "mercury telemetry".into(),
            at_utc: accents.start_utc.clone(),
            age_ms: Some(800),
        });
        accents.grid = Some("GG66pk".into());
        accents.name = Some("João Müller".into());
        accents.qth = Some("São Paulo".into());
        accents.rst_sent = Some("+1".into());
        accents.rst_rcvd = Some("-3".into());
        accents.my_power = Some("100".into());
        accents.notes = Some("Tested a photo, 12 kB".into());
        let mut bare = qso();
        bare.id = "q3".into();
        bare.peer = "W1AW/4".into();
        bare.start_utc = "2026-11-25T02:30:00.000Z".into();
        bare.end_utc = None;
        bare.band = None;
        bare.freq = None;
        bare.grid = None;
        bare.name = None;
        bare.qth = None;
        bare.rst_sent = None;
        bare.rst_rcvd = None;
        bare.my_power = None;
        bare.notes = None;
        bare.state = "draft".into();
        vec![full, accents, bare]
    }

    /// The sample export is pinned; `MC_WRITE_FIXTURE=1` rewrites it after a
    /// deliberate change (then the owner imports it into Log4OM again).
    #[test]
    fn log4om_sample_matches_the_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/export/log4om-sample.adi");
        let text = adif(&sample(), "0.5.0");
        if std::env::var_os("MC_WRITE_FIXTURE").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        let pinned = std::fs::read_to_string(&path).expect("the fixture exists").replace("\r\n", "\n");
        assert_eq!(pinned, text, "the sample export changed: check it, then MC_WRITE_FIXTURE=1");
    }

    #[test]
    fn mode_is_what_log4om_imports() {
        let f = fields(&adif(&[qso()], "0.5.0"));
        assert_eq!(get(&f, "MODE"), Some("DYNAMIC"));
        assert_eq!(get(&f, "SUBMODE"), None, "DYNAMIC's submodes are VARA's: none fits");
        assert_eq!(get(&f, "APP_MERCURYCHAT_MODE"), Some("MERCURY"));
    }
}
