//! A `--solve` mock that lists `linear-schedule` and reports the right linear
//! ladder, but as the SDK default (`reported_by` `"sdk"`), as a solver that
//! never overrides `Sampler::beta_ladder`. See `ladder_mock`.

mod ladder_mock;

fn main() -> std::process::ExitCode {
    ladder_mock::run("linear", "sdk")
}
