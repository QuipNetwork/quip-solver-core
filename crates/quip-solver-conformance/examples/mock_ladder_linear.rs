//! A `--solve` mock that lists `linear-schedule` and reports a linear ladder. See
//! `ladder_mock`.

mod ladder_mock;

fn main() -> std::process::ExitCode {
    ladder_mock::run("linear", "sampler")
}
