use std::process::ExitCode;

fn main() -> ExitCode {
    plaid_cli::main_entry(std::env::args_os())
}
