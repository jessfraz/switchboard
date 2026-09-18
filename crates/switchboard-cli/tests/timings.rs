use serde::Deserialize;
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use switchboard_core::ExecutionTimings;

#[derive(Deserialize)]
struct Executed {
    status: String,
    timings: ExecutionTimings,
}

#[test]
fn real_native_help_reports_measured_execution_phases() {
    let root = env::temp_dir().join(format!(
        "switchboard-timings-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after Unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("fixture operation should succeed");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let config = root.join("config.toml");
    let token = root.join("token");
    fs::write(&token, "offline-help-fixture").expect("fixture operation should succeed");
    fs::write(&config, format!("[secret.token]\nkind = 'file'\npath = {token:?}\n[auth.test]\nprovider = 'github'\nkind = 'github_token'\naccount = 'test'\ntoken = 'token'\n[namespace.github.test]\nprovider = 'github'\naccount = 'test'\nauth = 'test'\n")).expect("fixture operation should succeed");
    let binary_name = format!("gh{}", env::consts::EXE_SUFFIX);
    let gh = env::split_paths(&env::var_os("PATH").expect("PATH configured"))
        .map(|path| path.join(&binary_name))
        .find(|path| path.is_file())
        .expect("real gh required; CI installs it");
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_switchboard"))
        .arg("--config")
        .arg(&config)
        .args(["github.cli.read", "--ns", "github.test", "--json", "--", "--help"])
        .env("SWITCHBOARD_STATE_DB", root.join("operations.sqlite3"))
        .env("SWITCHBOARD_GH_BIN", gh)
        .env("SWITCHBOARD_OP_BIN", root.join("missing-op"))
        .output()
        .expect("fixture operation should succeed");
    let wall_us = ExecutionTimings::elapsed_us(started);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Executed = serde_json::from_slice(&output.stdout).expect("Switchboard should emit the documented JSON");
    assert_eq!(result.status, "executed");
    let timings = result.timings;
    let phases = [
        timings.locate_us,
        timings.probe_us,
        timings.materialize_us,
        timings.provider_us,
        timings.decode_us,
    ];
    let measured = phases
        .into_iter()
        .map(|phase| phase.expect("each real CLI phase measured"))
        .sum::<u64>();
    let adapter_us = timings.adapter_us.expect("adapter_us should be measured");
    let auth_us = timings.auth_us.expect("auth_us should be measured");
    let execution_us = timings.execution_us.expect("execution_us should be measured");
    assert!(timings.provider_us.expect("provider_us should be measured") > 0);
    assert!(timings.probe_us.expect("probe_us should be measured") > 0);
    assert!(adapter_us >= measured);
    assert!(execution_us >= adapter_us + auth_us);
    assert!(wall_us >= execution_us);
}
