//! Operator-facing number formatting for miner log lines.
//!
//! Energies stay in milli-units internally. These helpers change only the
//! text an operator reads.

/// Floor-divide a milli-energy to whole units for a log line.
///
/// Rust `/` truncates toward zero. Negative energies would then look better
/// than they are. Euclid division floors, which matches the operator spec.
#[must_use]
pub(crate) fn energy_units(milli: i64) -> i64 {
    milli.div_euclid(1000)
}

/// Render a millisecond duration with a rounded last unit.
///
/// | input | rendered |
/// | --- | --- |
/// | under 1 second | `889ms` |
/// | under 1 minute | `48.9s` |
/// | under 1 hour | `10m 49s` |
/// | 1 hour or more | `1h 12m` |
#[must_use]
pub(crate) fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        // Tenths of a second, rounded. 600 tenths is one minute.
        let tenths = (ms + 50) / 100;
        if tenths >= 600 {
            return "1m 0s".to_owned();
        }
        let secs = tenths / 10;
        let frac = tenths % 10;
        return format!("{secs}.{frac}s");
    }
    if ms < 3_600_000 {
        let mut mins = ms / 60_000;
        let rem = ms % 60_000;
        let mut secs = (rem + 500) / 1_000;
        if secs == 60 {
            mins = mins.saturating_add(1);
            secs = 0;
        }
        if mins >= 60 {
            return format_hours_minutes(mins / 60, mins % 60);
        }
        return format!("{mins}m {secs}s");
    }
    let mut hours = ms / 3_600_000;
    let rem = ms % 3_600_000;
    let mut mins = (rem + 30_000) / 60_000;
    if mins == 60 {
        hours = hours.saturating_add(1);
        mins = 0;
    }
    format_hours_minutes(hours, mins)
}

/// One debug line for a finished lease salt: effort spent and best energy.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "a flat record of independent log fields; a struct would only be built to be formatted"
)]
pub(crate) fn lease_salt_line(
    backend: &str,
    job: &str,
    salt_index: u64,
    best_energy_milli: Option<i64>,
    samples: usize,
    reads: u32,
    sweeps: u32,
    wall_ms: u64,
    device_ms: u64,
) -> String {
    let energy =
        best_energy_milli.map_or_else(|| "n/a".to_owned(), |e| energy_units(e).to_string());
    let wall = format_duration_ms(wall_ms);
    let device = format_duration_ms(device_ms);
    format!(
        "[quip-solver-{backend}] lease {job} salt {salt_index}: energy {energy}, samples {samples} \
         | reads={reads} sweeps={sweeps} | {wall} wall, {device} device"
    )
}

fn format_hours_minutes(hours: u64, mins: u64) -> String {
    format!("{hours}h {mins}m")
}

#[cfg(test)]
mod tests {
    use super::{energy_units, format_duration_ms, lease_salt_line};

    #[test]
    fn energy_units_floors_the_spec_table() {
        assert_eq!(energy_units(-14_369_000), -14_369);
        assert_eq!(energy_units(-14_535_322), -14_536);
        assert_eq!(energy_units(-14_536_604), -14_537);
        assert_eq!(energy_units(-14_513_000), -14_513);
        assert_eq!(energy_units(0), 0);
        assert_eq!(energy_units(1_500), 1);
    }

    #[test]
    fn format_duration_ms_follows_the_table() {
        let rows = [
            // The spec buckets.
            (889, "889ms"),
            (48_900, "48.9s"),
            (648_889, "10m 49s"),
            (4_320_000, "1h 12m"),
            // The last unit rounds.
            (0, "0ms"),
            (999, "999ms"),
            (1_000, "1.0s"),
            (48_850, "48.9s"),
            (155_002, "2m 35s"),
            (4_290_000, "1h 12m"),
            // The unit promotes when the last unit rounds up.
            (59_950, "1m 0s"),
            // 59m 59.5s is still under one hour, so seconds is the last unit.
            (3_599_500, "1h 0m"),
        ];
        for (ms, text) in rows {
            assert_eq!(format_duration_ms(ms), text, "{ms} ms");
        }
    }

    #[test]
    fn lease_salt_line_reports_effort_and_energy() {
        assert_eq!(
            lease_salt_line(
                "cpu",
                "ab12cd34..",
                7,
                Some(-4_957_000),
                64,
                64,
                4096,
                1_700,
                1_650
            ),
            "[quip-solver-cpu] lease ab12cd34.. salt 7: energy -4957, samples 64 \
             | reads=64 sweeps=4096 | 1.7s wall, 1.7s device"
        );
        assert_eq!(
            lease_salt_line("cpu", "ab12cd34..", 8, None, 0, 64, 4096, 889, 0),
            "[quip-solver-cpu] lease ab12cd34.. salt 8: energy n/a, samples 0 \
             | reads=64 sweeps=4096 | 889ms wall, 0ms device"
        );
    }
}
