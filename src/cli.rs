use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "oops",
    version,
    about = "You broke something. oops figures out what.",
    after_help = "oops only reads. It never changes your repository."
)]
pub struct Cli {
    /// Also show the repository state oops collected from git
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Emit structured JSON instead of human output
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Explain what oops currently sees, in a little more detail
    Explain,
}
