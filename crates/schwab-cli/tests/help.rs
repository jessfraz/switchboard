mod tests {
    use std::process::Command;

    #[test]
    fn help_does_not_print_credential_values() {
        let credentials = [
            ("SCHWAB_CLIENT_ID", "test-client-id-not-a-real-credential"),
            ("SCHWAB_CLIENT_SECRET", "test-client-secret-not-a-real-credential"),
            ("SCHWAB_ACCESS_TOKEN", "test-access-token-not-a-real-credential"),
            ("SCHWAB_REFRESH_TOKEN", "test-refresh-token-not-a-real-credential"),
        ];
        let commands: &[&[&str]] = &[
            &["--help"],
            &["-h"],
            &["accounts", "numbers", "--help"],
            &["accounts", "numbers", "-h"],
        ];

        for arguments in commands {
            let output = Command::new(env!("CARGO_BIN_EXE_schwab"))
                .env_clear()
                .envs(credentials)
                .args(*arguments)
                .output()
                .expect("run Schwab help without account configuration");
            assert!(output.status.success(), "help failed for {arguments:?}");
            let help = String::from_utf8(output.stdout).expect("help is UTF-8");
            let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
            assert!(help.contains("--client-secret"));
            for (name, value) in credentials {
                assert!(help.contains(name), "help omitted environment variable {name}");
                assert!(
                    !help.contains(value) && !stderr.contains(value),
                    "help exposed the value of {name} for {arguments:?}"
                );
            }
        }
    }
}
