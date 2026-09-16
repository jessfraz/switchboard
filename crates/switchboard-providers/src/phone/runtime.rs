use std::{
    env,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

use switchboard_core::{Error, ExecutionTarget, ResolvedCredentials, Result};

use crate::cli::{
    command::CliBinarySpec,
    locator::{CliLocator, DefaultCliLocator},
};

pub(crate) fn binary() -> CliBinarySpec {
    CliBinarySpec {
        program: "phone".into(),
        env_override: Some("SWITCHBOARD_PHONE_BIN".into()),
        version_args: vec!["--version".into()],
    }
}

pub(crate) fn command(target: &ExecutionTarget) -> Result<Command> {
    let state = state_dir(target.namespace.state_dir.as_deref())?;
    let mut command = Command::new(DefaultCliLocator.resolve(&binary())?);
    command.env_clear();
    // Keep runtime paths, never ambient credentials from another namespace.
    for name in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SYSTEMROOT",
        "LANG",
        "LC_ALL",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NIX_SSL_CERT_FILE",
        "UV_CACHE_DIR",
        "XDG_CACHE_HOME",
    ] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("PHONE_CONFIG", state.join("config.toml"));
    command.env("PHONE_STATE_DIR", state.join("calls"));
    command.env("PHONE_SUPERVISOR_PID", std::process::id().to_string());
    let ResolvedCredentials::PhoneCli {
        api_key,
        api_secret,
        model_api_key,
    } = &target.credentials
    else {
        return Err(Error::UnsupportedOperation(
            "phone requires phone_cli credentials".into(),
        ));
    };
    if let Some(value) = api_key {
        command.env("PHONE_API_KEY", value.expose());
    }
    if let Some(value) = api_secret {
        command.env("PHONE_API_SECRET", value.expose());
    }
    if let Some(value) = model_api_key {
        command.env("PHONE_MODEL_API_KEY", value.expose());
    }
    command.arg("--json");
    Ok(command)
}

pub(crate) fn state_dir(state: Option<&Path>) -> Result<&Path> {
    state.filter(|path| path.is_absolute()).ok_or_else(|| {
        Error::Config(
            "phone namespace requires an absolute state_dir for isolated configuration and transcripts".into(),
        )
    })
}

/// A private socket carries the brief without an argv entry or plaintext file.
#[cfg(unix)]
pub(crate) fn capture(mut command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output> {
    use std::{
        io::Write,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
        process::Stdio,
    };
    let (read, mut write) =
        UnixStream::pair().map_err(|_| Error::Execution("could not open private phone input".into()))?;
    write
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| Error::Execution("could not bound phone input".into()))?;
    let writer = std::thread::spawn(move || {
        write.write_all(&input)?;
        write.shutdown(Shutdown::Write)
    });
    let descriptor: OwnedFd = read.into();
    let result = switchboard_core::process::output_with_stdin_timeout(&mut command, Stdio::from(descriptor), timeout);
    // Drop the command's stdin handle even if spawn failed, before joining.
    drop(command);
    let written = writer
        .join()
        .map_err(|_| Error::Execution("phone input writer stopped".into()))?;
    let output = result.map_err(|_| Error::Execution("phone process stopped without a confirmed result; inspect its encrypted call record before attempting another call".into()))?;
    written.map_err(|_| Error::Execution("could not deliver the phone request".into()))?;
    Ok(output)
}

#[cfg(not(unix))]
pub(crate) fn capture(_command: Command, _input: Vec<u8>, _timeout: Duration) -> Result<Output> {
    Err(Error::UnsupportedOperation(
        "phone currently requires macOS or Linux".into(),
    ))
}
