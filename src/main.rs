use clap::Parser;
use papercut::cli::Cli;

fn main() {
    let cli = Cli::parse();
    let code = papercut::run(cli);
    std::process::exit(code);
}
