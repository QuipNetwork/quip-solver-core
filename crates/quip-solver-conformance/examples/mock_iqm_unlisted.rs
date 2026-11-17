//! A `--solve` mock that solves the IQM form without listing `IQM_SAMPLE`. See `iqm_mock`.

mod iqm_mock;

fn main() -> std::process::ExitCode {
    iqm_mock::run(false, 0)
}
