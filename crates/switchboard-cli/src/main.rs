use switchboard_cli::{args_from_env, main_entry};

fn main() -> std::process::ExitCode {
    main_entry(args_from_env())
}
