//! CLI entry for the mock coordinator: run one miner binary through the
//! full conformance session and print the driver report.

use quip_solver_conformance::driver::drive_miner;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(bin_path) = args.next() else {
        #[expect(
            clippy::print_stderr,
            reason = "CLI usage error is intentionally written to stderr"
        )]
        {
            eprintln!("usage: quip-solver-drive <solver-bin> <unix://socket>");
        }
        return ExitCode::from(64);
    };
    let Some(socket) = args.next() else {
        #[expect(
            clippy::print_stderr,
            reason = "CLI usage error is intentionally written to stderr"
        )]
        {
            eprintln!("usage: quip-solver-drive <solver-bin> <unix://socket>");
        }
        return ExitCode::from(64);
    };
    let report = drive_miner(&bin_path, &socket).await;
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
    if report.is_conformant() {
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
