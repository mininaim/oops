use clap::Parser;
use oops::cli::{Cli, Command};
use oops::git::SystemGit;
use oops::output::human::Mode;
use oops::output::spinner::Spinner;
use oops::output::{Style, human, json};
use oops::{diagnosis, git, output};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("oops: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    // The spinner animates on stderr only for interactive human output,
    // and only if inspection outlasts a short grace period. finish() erases
    // it before anything is printed — including the error path below.
    let spinner = Spinner::start(!cli.json && output::detect_animation());
    let snapshot = git::snapshot::collect(&SystemGit);
    spinner.finish();
    let snapshot = snapshot?;

    let diagnoses = diagnosis::diagnose(&snapshot);

    if cli.json {
        println!("{}", json::render(&snapshot, &diagnoses)?);
        return Ok(());
    }

    let mode = match (&cli.command, cli.verbose) {
        (Some(Command::Explain), _) => Mode::Explain,
        (None, true) => Mode::Verbose,
        (None, false) => Mode::Default,
    };
    print!(
        "{}",
        human::render(&snapshot, &diagnoses, mode, &Style::detect())
    );
    Ok(())
}
