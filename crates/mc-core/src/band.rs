//! Amateur bands by dial frequency, 160 m to 6 m (M3a; the logbook and M3b
//! reuse it). Edges are inclusive. A frequency outside every band has none:
//! no band is guessed.

/// The amateur bands MercuryChat names, lowest first: (low Hz, high Hz, name).
/// Names are ADIF's band names.
pub const BANDS: [(u64, u64, &str); 11] = [
    (1_800_000, 2_000_000, "160m"),
    (3_500_000, 4_000_000, "80m"),
    (5_250_000, 5_450_000, "60m"),
    (7_000_000, 7_300_000, "40m"),
    (10_100_000, 10_150_000, "30m"),
    (14_000_000, 14_350_000, "20m"),
    (18_068_000, 18_168_000, "17m"),
    (21_000_000, 21_450_000, "15m"),
    (24_890_000, 24_990_000, "12m"),
    (28_000_000, 29_700_000, "10m"),
    (50_000_000, 54_000_000, "6m"),
];

/// The band a dial frequency in Hz falls in, edges included.
pub fn band_for(hz: u64) -> Option<&'static str> {
    BANDS.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&hz)).map(|(_, _, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_edge_is_inclusive_and_one_hertz_outside_is_no_band() {
        let expected = [
            (1_800_000, 2_000_000, "160m"),
            (3_500_000, 4_000_000, "80m"),
            (5_250_000, 5_450_000, "60m"),
            (7_000_000, 7_300_000, "40m"),
            (10_100_000, 10_150_000, "30m"),
            (14_000_000, 14_350_000, "20m"),
            (18_068_000, 18_168_000, "17m"),
            (21_000_000, 21_450_000, "15m"),
            (24_890_000, 24_990_000, "12m"),
            (28_000_000, 29_700_000, "10m"),
            (50_000_000, 54_000_000, "6m"),
        ];
        for (lo, hi, name) in expected {
            assert_eq!(band_for(lo), Some(name), "{lo} Hz");
            assert_eq!(band_for(hi), Some(name), "{hi} Hz");
            assert_eq!(band_for((lo + hi) / 2), Some(name), "middle of {name}");
            assert_eq!(band_for(lo - 1), None, "{} Hz is below {name}", lo - 1);
            assert_eq!(band_for(hi + 1), None, "{} Hz is above {name}", hi + 1);
        }
    }

    #[test]
    fn outside_every_band_is_none() {
        for hz in [0, 1, 136_000, 1_000_000, 2_500_000, 11_000_000, 27_000_000, 144_000_000, u64::MAX] {
            assert_eq!(band_for(hz), None, "{hz} Hz");
        }
    }
}
