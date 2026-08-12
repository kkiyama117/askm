//! `askm` binary entry point.
//!
//! Argument parsing lives in `cli`, command logic in `commands`; this file
//! stays thin: parse, dispatch, and map errors to a single readable line
//! (the full `anyhow` context chain, joined, not a `{:?}` debug dump) and a
//! non-zero exit code.

mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match commands::run(cli) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code.clamp(0, u8::MAX as i32) as u8),
        Err(err) => {
            eprintln!("askm: error: {}", format_error_chain(&err));
            ExitCode::FAILURE
        }
    }
}

/// Join an anyhow error and every one of its causes into a single readable
/// line, e.g. "installing demo@official: no plugin named ...".
fn format_error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}
