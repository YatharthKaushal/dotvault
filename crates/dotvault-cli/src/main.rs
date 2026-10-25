mod cli;
mod client;
mod clipboard;
mod commands;
mod config;
mod discover;
mod filter;
mod fsutil;
mod history;
mod index;
mod keys;
mod output;
mod prompt;
mod resolve;
mod scan;
mod session;

use std::io;
use std::process::ExitCode;

use clap::Parser;
use owo_colors::OwoColorize;

fn main() -> ExitCode {
    let cli = cli::Cli::parse_from(cli::expand_shorthands(std::env::args_os()));
    match commands::run(cli) {
        Ok(code) => code,
        Err(err) => {
            // `dotvault ls | head` closes the pipe early; that is not an error.
            let broken_pipe = err
                .chain()
                .filter_map(|c| c.downcast_ref::<io::Error>())
                .any(|e| e.kind() == io::ErrorKind::BrokenPipe);
            if broken_pipe {
                return ExitCode::SUCCESS;
            }
            // Errors can carry text from a server or a share, so they are sanitized too.
            anstream::eprintln!(
                "{} {}",
                "error:".red().bold(),
                output::sanitize(&format!("{err:#}"))
            );
            ExitCode::from(1)
        }
    }
}
