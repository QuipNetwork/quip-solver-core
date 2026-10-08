//! CLI entry for the conformance gates.
//!
//! `quip-solver-drive <solver-bin> <unix://socket>` runs the session walk
//! against the mock coordinator. `quip-solver-drive --solve <solver-bin>` runs
//! the one-shot `--solve` gate.

use quip_solver_conformance::driver::drive_miner;
use quip_solver_conformance::solve::drive_solve;
use std::process::ExitCode;

const USAGE: &str =
    "usage: quip-solver-drive <solver-bin> <unix://socket>\n       quip-solver-drive --solve <solver-bin>";

fn usage() -> ExitCode {
    #[expect(
        clippy::print_stderr,
        reason = "CLI usage error is intentionally written to stderr"
    )]
    {
        eprintln!("{USAGE}");
    }
    ExitCode::from(64)
}

fn verdict(conformant: bool, bin_path: &str) -> ExitCode {
    if conformant {
        ExitCode::SUCCESS
    } else {
        #[expect(
            clippy::print_stderr,
            reason = "CLI failure summary is intentionally written to stderr"
        )]
        {
            eprintln!("conformance failed for {bin_path}");
        }
        ExitCode::from(1)
    }
}

async fn session(bin_path: &str, socket: &str) -> ExitCode {
    let report = drive_miner(bin_path, socket).await;
    #[expect(
        clippy::print_stdout,
        reason = "user-facing CLI prints the conformance report"
    )]
    {
        println!("conformance report for {bin_path}:");
        print!("{}", report.summary());
    }
    #[expect(
        clippy::print_stderr,
        reason = "the CLI shows the miner's own log after the report"
    )]
    {
        if !report.stderr.is_empty() {
            eprintln!("miner stderr:\n{}", report.stderr);
        }
    }
    verdict(report.is_conformant(), bin_path)
}

async fn solve(bin_path: &str) -> ExitCode {
    let report = drive_solve(bin_path).await;
    #[expect(
        clippy::print_stdout,
        reason = "user-facing CLI prints the conformance report"
    )]
    {
        println!("--solve conformance report for {bin_path}:");
        print!("{}", report.summary());
    }
    verdict(report.is_conformant(), bin_path)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, bin_path] if flag == "--solve" => solve(bin_path).await,
        [bin_path, socket] => session(bin_path, socket).await,
        _ => usage(),
    }
}
