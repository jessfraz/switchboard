use clap::CommandFactory;

use crate::Cli;

#[test]
fn root_help_prefers_ucla_preset_login() {
    let mut command = Cli::command();
    let mut help = Vec::new();
    command.write_help(&mut help).expect("help should render");
    let help = String::from_utf8(help).expect("help should be valid utf-8");

    assert!(help.contains("mychart login ucla"));
    assert!(help.contains("mychart finish '<auth-code>'  # fallback"));
    assert!(!help.contains("mychart auth exchange-url '<auth-code>'"));
    assert!(!help.contains("mychart auth login --base-url"));
    assert!(!help.contains("mychart auth login --dynamic-client"));
    assert!(!help.contains("--redirect-uri https://example.org/mychart-callback/"));
}
