//! Suggested frequency-list presets per IARU region (M3b, D15).
//!
//! REVIEW BEFORE RELEASE: drafted 18 November 2026 from the IARU Region 1, 2
//! and 3 HF band plans' digital-mode segments, not official. Each is a dial
//! frequency for USB data (PKTUSB), chosen inside a digital segment and clear
//! of the well-known FT8, FT4, JS8, WSPR and beacon spots. They are labelled
//! "suggested" wherever they appear, and the operator may edit or delete any
//! of them (06 D15).

/// One shipped suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Stable across releases: how "Restore suggested" recognises it.
    pub key: &'static str,
    pub name: &'static str,
    pub freq_hz: u64,
    pub mode: &'static str,
    pub purpose: &'static [&'static str],
}

const ALL: &[&str] = &["beacon", "cq", "chat"];

const fn p(key: &'static str, name: &'static str, freq_hz: u64) -> Preset {
    Preset { key, name, freq_hz, mode: "PKTUSB", purpose: ALL }
}

const REGION_1: &[Preset] = &[
    p("r1-80m", "80 m data (suggested)", 3_595_000),
    p("r1-40m", "40 m data (suggested)", 7_047_000),
    p("r1-30m", "30 m data (suggested)", 10_145_000),
    p("r1-20m-a", "20 m data (suggested)", 14_090_000),
    p("r1-20m-b", "20 m data, automatic stations (suggested)", 14_105_000),
    p("r1-17m", "17 m data (suggested)", 18_106_000),
    p("r1-15m", "15 m data (suggested)", 21_095_000),
    p("r1-12m", "12 m data (suggested)", 24_925_000),
    p("r1-10m", "10 m data (suggested)", 28_120_000),
];

const REGION_2: &[Preset] = &[
    p("r2-80m", "80 m data (suggested)", 3_585_000),
    p("r2-40m", "40 m data (suggested)", 7_105_000),
    p("r2-30m", "30 m data (suggested)", 10_145_000),
    p("r2-20m-a", "20 m data (suggested)", 14_090_000),
    p("r2-20m-b", "20 m data, automatic stations (suggested)", 14_105_000),
    p("r2-17m", "17 m data (suggested)", 18_106_000),
    p("r2-15m", "15 m data (suggested)", 21_095_000),
    p("r2-12m", "12 m data (suggested)", 24_925_000),
    p("r2-10m", "10 m data (suggested)", 28_120_000),
];

const REGION_3: &[Preset] = &[
    p("r3-80m", "80 m data (suggested)", 3_590_000),
    p("r3-40m", "40 m data (suggested)", 7_045_000),
    p("r3-30m", "30 m data (suggested)", 10_145_000),
    p("r3-20m-a", "20 m data (suggested)", 14_090_000),
    p("r3-20m-b", "20 m data, automatic stations (suggested)", 14_105_000),
    p("r3-17m", "17 m data (suggested)", 18_106_000),
    p("r3-15m", "15 m data (suggested)", 21_095_000),
    p("r3-12m", "12 m data (suggested)", 24_925_000),
    p("r3-10m", "10 m data (suggested)", 28_120_000),
];

/// The suggestions for IARU region 1, 2 or 3; none for any other number.
pub fn presets(region: u8) -> Vec<Preset> {
    match region {
        1 => REGION_1.to_vec(),
        2 => REGION_2.to_vec(),
        3 => REGION_3.to_vec(),
        _ => vec![],
    }
}
