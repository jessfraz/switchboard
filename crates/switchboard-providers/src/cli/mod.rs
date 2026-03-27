mod command;
mod executor;
mod locator;
mod manifest;
pub(crate) mod passthrough;
mod probe;
mod runtime;

pub(crate) use crate::cli::{
    command::{CliCommandSpec, CliResponse},
    manifest::{CliCommandHandler, CliProviderCatalog},
    runtime::{CliProviderBackend, CliRuntimeMaterializer},
};

#[cfg(test)]
mod tests {
    use crate::cli::{
        command::{CliBinarySpec, CliCapabilityProbe},
        executor::ProcessCliExecutor,
        locator::{CliLocator, DefaultCliLocator},
        probe::{CliProbe, DefaultCliProbe},
    };

    #[test]
    fn probes_real_gh_binary() {
        let version = probe_real_binary(
            CliBinarySpec {
                program: "gh".to_owned(),
                env_override: Some("SWITCHBOARD_GH_BIN".to_owned()),
                version_args: vec!["--version".to_owned()],
            },
            CliCapabilityProbe {
                id: "gh-api".to_owned(),
                args: vec!["api".to_owned(), "--help".to_owned()],
            },
        );

        assert!(
            version.contains("gh version"),
            "unexpected gh version output: {version}"
        );
    }

    #[test]
    fn probes_real_gws_binary() {
        let version = probe_real_binary(
            CliBinarySpec {
                program: "gws".to_owned(),
                env_override: Some("SWITCHBOARD_GWS_BIN".to_owned()),
                version_args: vec!["--version".to_owned()],
            },
            CliCapabilityProbe {
                id: "gws-calendar".to_owned(),
                args: vec!["calendar".to_owned(), "--help".to_owned()],
            },
        );

        assert!(version.starts_with("gws "), "unexpected gws version output: {version}");
    }

    fn probe_real_binary(binary: CliBinarySpec, capability: CliCapabilityProbe) -> String {
        let locator = DefaultCliLocator;
        let program = locator.resolve(&binary).expect("binary should resolve");
        let probe = DefaultCliProbe::default();
        let executor = ProcessCliExecutor;

        probe
            .inspect(&binary, &program, &capability, &executor)
            .expect("binary should be probeable")
    }
}
