use momence_cli::main_entry;

fn main() -> std::process::ExitCode {
    main_entry(std::env::args_os())
}
