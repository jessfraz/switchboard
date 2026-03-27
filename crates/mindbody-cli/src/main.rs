use std::process::ExitCode;

fn main() -> ExitCode {
    mindbody_cli::main_entry(std::env::args_os())
}
