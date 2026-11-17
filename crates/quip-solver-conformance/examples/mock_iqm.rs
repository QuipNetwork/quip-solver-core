//! A `--solve` mock that lists `IQM_SAMPLE` and scores every read. See `iqm_mock`.

mod iqm_mock;

fn main() -> std::process::ExitCode {
    iqm_mock::run(true, 0)
}
