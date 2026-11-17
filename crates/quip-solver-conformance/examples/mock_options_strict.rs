//! A `--solve` mock that advertises two options and validates them. See
//! `options_mock`.

mod options_mock;

fn main() -> std::process::ExitCode {
    options_mock::run(true)
}
