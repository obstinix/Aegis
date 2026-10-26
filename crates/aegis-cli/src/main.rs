//! `aegis`: the Aegis command-line interface (ADR-0009).
//!
//! Commands:
//! - `aegis doctor`: human-readable host capability report.
//! - `aegis doctor --json`: the same report as JSON (schema `aegis.doctor/1`).
//! - `aegis doctor --raw`: the raw observations behind the report, as JSON,
//!   for bug reports and test fixtures.
//! - `aegis run --config <vm.toml>`: boot a Linux guest under KVM (Linux
//!   hosts). Guest serial output goes to stdout with control characters
//!   escaped, and a summary goes to stderr. Exits 0 if the guest stopped
//!   normally, 1 if it failed.
//! - `aegis selftest seccomp`: install the run-loop seccomp filter and
//!   attempt a forbidden syscall; the process must be killed by `SIGSYS`.
//! - `aegis version`.
//!
//! Usage and configuration errors exit with status 2.

mod config;
mod render;
mod vmrun;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use aegis_probe::report::{Observations, Report};

const USAGE: &str = "\
usage: aegis <command>

commands:
  doctor [--json | --raw]    report what this host can run
  run --config <vm.toml>     boot a Linux guest under KVM (Linux hosts)
  selftest seccomp           check that the run-loop seccomp filter kills
  version                    print the version
  help                       print this message";

enum Command {
    Doctor(DoctorOutput),
    Run(PathBuf),
    SelftestSeccomp,
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
        ["run", "--config", path] => Ok(Command::Run(PathBuf::from(path))),
        ["run", ..] => Err("run takes --config <vm.toml>".to_owned()),
        ["selftest", "seccomp"] => Ok(Command::SelftestSeccomp),
        ["selftest", ..] => Err("selftest takes: seccomp".to_owned()),
        [other, ..] => Err(format!("unknown command `{other}`")),
    }
}

fn text_command(command: Command) -> Result<String, String> {
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
        Command::SelftestSeccomp => vmrun::selftest_seccomp().map(|()| String::new()),
        Command::Run(_) => Err("internal: run is not a text command".to_owned()),
    }
}

fn usage_error(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "aegis: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

fn run_command(path: &std::path::Path) -> ExitCode {
    let report = match config::load(path).and_then(|cfg| vmrun::run_vm(&cfg)) {
        Ok(report) => report,
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "aegis: {message}");
            return ExitCode::from(2);
        }
    };
    // A closed stdout is not worth a second error; the summary still goes to stderr.
    let _ = write!(
        std::io::stdout(),
        "{}",
        vmrun::escape_for_terminal(&report.serial)
    );
    if let Some(hvc) = &report.virtio_console {
        let _ = write!(
            std::io::stdout(),
            "\n--- virtio-console (hvc0) ---\n{}",
            vmrun::escape_for_terminal(hvc)
        );
    }
    let _ = writeln!(std::io::stderr(), "{}", report.summary);
    if report.normal {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => return usage_error(&message),
    };
    if let Command::Run(path) = &command {
        return run_command(path);
    }
    match text_command(command) {
        Ok(text) => {
            // A closed stdout (e.g. `aegis doctor | head`) is not an error worth reporting.
            if !text.is_empty() {
                let _ = writeln!(std::io::stdout(), "{text}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "aegis: {message}");
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
        assert!(
            matches!(parse(&args(&["run", "--config", "vm.toml"])), Ok(Command::Run(p)) if p.as_os_str() == "vm.toml")
        );
        assert!(matches!(
            parse(&args(&["selftest", "seccomp"])),
            Ok(Command::SelftestSeccomp)
        ));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&args(&["doctor", "--json", "--raw"])).is_err());
        assert!(parse(&args(&["doctor", "--yaml"])).is_err());
        assert!(parse(&args(&["launch"])).is_err());
        assert!(parse(&args(&["run"])).is_err());
        assert!(parse(&args(&["run", "vm.toml"])).is_err());
        assert!(parse(&args(&["run", "--config"])).is_err());
        assert!(parse(&args(&["selftest"])).is_err());
        assert!(parse(&args(&["selftest", "network"])).is_err());
    }
}
