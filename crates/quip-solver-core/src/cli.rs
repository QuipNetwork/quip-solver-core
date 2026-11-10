//! CLI flags shared by every miner binary.
//!
//! Each binary defines its own `clap::Parser` struct that flattens
//! [`CommonArgs`] and adds any backend-specific flags (device index,
//! utilization ceiling, …).

use clap::Args;

/// Verbosity words `--log-level` accepts, matching `logging::LEVELS`.
///
/// Duplicated rather than shared because `logging` owns the runtime meaning of
/// each word and this list owns the *surface*: clap uses it to reject a typo
/// during argument parsing, name the accepted values in the error, and list
/// them in `--help`. `logging::resolve_directives` still re-checks the level it
/// is handed, since it is also reachable from a caller that never parsed a
/// command line. `every_level_the_parser_accepts_is_one_the_logger_accepts`
/// keeps the two lists from drifting apart.
const LOG_LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

/// Flags every miner accepts. Flatten into a binary's `Cli` with
/// `#[command(flatten)]`.
#[derive(Args, Debug, Clone)]
pub struct CommonArgs {
    /// Coordinator endpoint, e.g. `unix:///run/quip/coord.sock`.
    #[arg(long)]
    pub quip_coordinator: Option<String>,
    /// Miner id in Hello / Status. Defaults to `<backend>-0`.
    #[arg(long)]
    pub miner_id: Option<String>,
    // `--capabilities` / `--solve` / `--check` each print an answer and exit,
    // so passing two is a caller mistake rather than something to resolve by
    // precedence. `session::run_code` tests them in declaration order, which
    // meant `--check --capabilities` printed capabilities and never opened the
    // device: a broken node reporting healthy. clap rejects the pair instead.
    /// Print the capabilities JSON and exit.
    #[arg(long, conflicts_with_all = ["solve", "check"])]
    pub capabilities: bool,
    /// Read one problem as JSON on stdin, write its solutions as JSON on
    /// stdout, and exit. The mode a one-shot caller uses: no coordinator, no
    /// session, no credits.
    #[arg(long, conflicts_with_all = ["capabilities", "check"])]
    pub solve: bool,
    /// Probe that the backend is runnable and exit.
    #[arg(long, conflicts_with_all = ["capabilities", "solve"])]
    pub check: bool,
    /// Log level (accepted for compatibility; stderr is the default sink).
    #[arg(long, default_value = "info",
          value_parser = clap::builder::PossibleValuesParser::new(LOG_LEVELS))]
    pub log_level: String,
    /// Sweeps per beta rung in the annealing schedule (>= 1). Overrides the
    /// default of 1 without going through the coordinator; e.g. raise it to
    /// converge Gibbs further. A miner-local setting for now (not on the wire).
    ///
    /// The `>= 1` in that sentence is now enforced: backends divide the sweep
    /// budget by this, so a zero is a divide-by-zero in the backend rather than
    /// a schedule with no rungs.
    #[arg(long, value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub sweeps_per_beta: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::{CommonArgs, LOG_LEVELS};
    use clap::Parser;

    /// The three one-shot flags, which must be mutually exclusive.
    const MODE_FLAGS: [&str; 3] = ["capabilities", "solve", "check"];

    /// `CommonArgs` is only ever flattened into a binary's own `Cli`, so the
    /// tests exercise it the same way.
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(flatten)]
        common: CommonArgs,
    }

    fn parse(args: &[&str]) -> Result<CommonArgs, clap::Error> {
        TestCli::try_parse_from(std::iter::once("miner").chain(args.iter().copied()))
            .map(|c| c.common)
    }

    #[test]
    fn each_mode_flag_is_accepted_on_its_own() {
        for flag in MODE_FLAGS {
            let arg = format!("--{flag}");
            assert!(parse(&[&arg]).is_ok(), "--{flag} alone must parse");
        }
    }

    #[test]
    fn every_documented_log_level_parses() {
        for level in LOG_LEVELS {
            let got = parse(&["--log-level", level]).expect("documented level must parse");
            assert_eq!(got.log_level, level);
        }
    }

    /// The clap surface and the logger's own check must accept the same words.
    /// `logging` keeps its own private list, so this drives the real entry
    /// point: a level clap advertises but `init` refuses would be a flag that
    /// parses and then kills the miner at startup. `init` uses `try_init`
    /// internally, so repeated calls are harmless.
    #[test]
    fn every_level_the_parser_accepts_is_one_the_logger_accepts() {
        for level in LOG_LEVELS {
            assert!(
                crate::logging::init(level).is_ok(),
                "--log-level {level} parses but the logger rejects it"
            );
        }
    }

    #[test]
    fn sweeps_per_beta_accepts_one_and_above() {
        for n in ["1", "3", "4096"] {
            let got = parse(&["--sweeps-per-beta", n]).expect("a positive value must parse");
            assert_eq!(got.sweeps_per_beta, Some(n.parse().expect("test constant")));
        }
    }

    #[test]
    fn defaults_with_no_arguments() {
        let got = parse(&[]).expect("no args must parse");
        assert_eq!(got.log_level, "info");
        assert_eq!(got.sweeps_per_beta, None);
    }

    /// Without the mode-flag rows, `session::run_code`'s precedence order
    /// decides silently: `--check --capabilities` printed capabilities and
    /// never opened the device, so a broken node reported healthy.
    /// The misspelling now fails at argument parsing rather than at subscriber
    /// setup, so the accepted values appear in the error and in `--help`.
    /// A zero `--sweeps-per-beta` reaches a backend as a divisor; the doc
    /// comment promised `>= 1` and the parser used to take anything.
    #[test]
    fn invalid_arguments_report_the_right_clap_error() {
        use clap::error::ErrorKind;
        let check = |args: &[&str], kind: ErrorKind| {
            let label = args.join(" ");
            let Err(err) = parse(args) else {
                panic!("{label} must be rejected");
            };
            assert_eq!(err.kind(), kind, "{label}");
        };
        for (i, a) in MODE_FLAGS.iter().enumerate() {
            for b in MODE_FLAGS.iter().skip(i + 1) {
                check(
                    &[&format!("--{a}"), &format!("--{b}")],
                    ErrorKind::ArgumentConflict,
                );
            }
        }
        let rows = [
            (&["--log-level", "infoo"], ErrorKind::InvalidValue),
            (&["--sweeps-per-beta", "0"], ErrorKind::ValueValidation),
        ];
        for (args, kind) in rows {
            check(args, kind);
        }
    }
}
