//! A `--solve` mock that lists `IQM_SAMPLE` and reports one milli too much. See `iqm_mock`.

mod iqm_mock;

fn main() -> std::process::ExitCode {
    iqm_mock::run(true, 1)
}
