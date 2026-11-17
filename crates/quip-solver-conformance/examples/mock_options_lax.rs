//! A `--solve` mock that advertises two options and accepts any `options` object. See
//! `options_mock`.

mod options_mock;

fn main() -> std::process::ExitCode {
    options_mock::run(false)
}
