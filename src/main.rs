use std::io::{self, ErrorKind};
use std::process::ExitCode;

use clap::Parser;
use proofreader::cli::{Cli, run};

/// Parses the command line, runs the linter and maps the outcome to an exit code; a closed
/// stdout (for example when piped into `head`) ends the run quietly.
fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::from(2)
        }
    }
}
