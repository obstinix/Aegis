//! `aegis`: the Aegis command-line interface (ADR-0009).
//!
//! Commands:
//! - `aegis doctor`: human-readable host capability report.
//! - `aegis doctor --json`: the same report as JSON (schema `aegis.doctor/1`).
//! - `aegis doctor --raw`: the raw observations behind the report, as JSON,
//!   for bug reports and test fixtures.
//! - `aegis version`.

mod render;

use std::io::Write as _;
use std::process::ExitCode;

use aegis_probe::report::{Observations, Report};

const USAGE: &str = "\
usage: aegis <command>

commands:
  doctor [--json | --raw]   report what this host can run
  version                   print the version
  help                      print this message";

enum Command {
    Doctor(DoctorOutput),
    Version,
    Help,
}

enum DoctorOutput {
    Human,
    Json,
    Raw,
}

fn parse(args: &[String]) -> Result<Command, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => Ok(Command::Help),
        ["version" | "--version" | "-V"] => Ok(Command::Version),
        ["doctor"] => Ok(Command::Doctor(DoctorOutput::Human)),
        ["doctor", "--json"] => Ok(Command::Doctor(DoctorOutput::Json)),
        ["doctor", "--raw"] => Ok(Command::Doctor(DoctorOutput::Raw)),
        ["doctor", ..] => Err("doctor takes at most one of --json, --raw".to_owned()),
        [other, ..] => Err(format!("unknown command `{other}`")),
    }
}

fn run(command: Command) -> Result<String, String> {
    let json_err = |e: serde_json::Error| format!("JSON encoding failed: {e}");
    match command {
        Command::Help => Ok(USAGE.to_owned()),
        Command::Version => Ok(format!("aegis {}", env!("CARGO_PKG_VERSION"))),
        Command::Doctor(output) => {
            let obs = Observations::collect();
            match output {
                DoctorOutput::Raw => serde_json::to_string_pretty(&obs).map_err(json_err),
                DoctorOutput::Json => {
                    serde_json::to_string_pretty(&Report::evaluate(&obs)).map_err(json_err)
                }
                DoctorOutput::Human => Ok(render::human(&Report::evaluate(&obs))),
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&args).and_then(run);
    match result {
        Ok(text) => {
            // A closed stdout (e.g. `aegis doctor | head`) is not an error worth reporting.
            let _ = writeln!(std::io::stdout(), "{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "aegis: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_commands() {
        assert!(matches!(parse(&args(&[])), Ok(Command::Help)));
        assert!(matches!(parse(&args(&["--version"])), Ok(Command::Version)));
        assert!(matches!(
            parse(&args(&["doctor"])),
            Ok(Command::Doctor(DoctorOutput::Human))
        ));
        assert!(matches!(
            parse(&args(&["doctor", "--json"])),
            Ok(Command::Doctor(DoctorOutput::Json))
        ));
        assert!(matches!(
            parse(&args(&["doctor", "--raw"])),
            Ok(Command::Doctor(DoctorOutput::Raw))
        ));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&args(&["doctor", "--json", "--raw"])).is_err());
        assert!(parse(&args(&["doctor", "--yaml"])).is_err());
        assert!(parse(&args(&["launch"])).is_err());
    }
}
