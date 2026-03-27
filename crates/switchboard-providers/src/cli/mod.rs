mod command;
mod executor;
mod locator;
pub(crate) mod passthrough;
mod probe;
mod runtime;

pub(crate) use crate::cli::{
    command::{CliBinarySpec, CliCapabilityProbe, CliCommandSpec, CliResponse},
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
                program: "gh",
                env_override: Some("SWITCHBOARD_GH_BIN"),
                version_args: &["--version"],
            },
            CliCapabilityProbe {
                name: "gh-api",
                args: &["api", "--help"],
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
                program: "gws",
                env_override: Some("SWITCHBOARD_GWS_BIN"),
                version_args: &["--version"],
            },
            CliCapabilityProbe {
                name: "gws-calendar",
                args: &["calendar", "--help"],
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
