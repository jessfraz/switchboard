use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Mutex,
};

use switchboard_core::{Error, Result};

use crate::{
    cli::{
        command::{CliBinarySpec, CliCapabilityProbe},
        executor::{CliExecutor, CliInvocation, CliStdioMode},
    },
    process_runtime::ProcessContext,
};

pub(crate) trait CliProbe: Send + Sync {
    fn inspect(
        &self,
        binary: &CliBinarySpec,
        program: &Path,
        capability: &CliCapabilityProbe,
        executor: &dyn CliExecutor,
    ) -> Result<String>;
}

#[derive(Default)]
pub(crate) struct DefaultCliProbe {
    versions: Mutex<BTreeMap<String, String>>,
    capabilities: Mutex<BTreeSet<String>>,
}

impl CliProbe for DefaultCliProbe {
    fn inspect(
        &self,
        binary: &CliBinarySpec,
        program: &Path,
        capability: &CliCapabilityProbe,
        executor: &dyn CliExecutor,
    ) -> Result<String> {
        let version = self.cached_version(binary, program, executor)?;
        self.ensure_capability(program, capability, executor)?;
        Ok(version)
    }
}

impl DefaultCliProbe {
    fn cached_version(&self, binary: &CliBinarySpec, program: &Path, executor: &dyn CliExecutor) -> Result<String> {
        let key = program.display().to_string();
        if let Some(version) = self
            .versions
            .lock()
            .ok()
            .and_then(|versions| versions.get(&key).cloned())
        {
            return Ok(version);
        }

        let output = executor.execute(CliInvocation {
            program: program.to_path_buf(),
            args: binary.version_args.clone(),
            runtime: ProcessContext::new(),
            stdio_mode: CliStdioMode::Capture,
        })?;
        let version = output
            .stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .ok_or_else(|| {
                Error::Execution(format!(
                    "{} --version produced no usable version output",
                    program.display()
                ))
            })?
            .to_owned();

        match self.versions.lock() {
            Ok(mut versions) => {
                versions.insert(key, version.clone());
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(key, version.clone());
            }
        }

        Ok(version)
    }

    fn ensure_capability(
        &self,
        program: &Path,
        capability: &CliCapabilityProbe,
        executor: &dyn CliExecutor,
    ) -> Result<()> {
        let key = format!("{}::{}", program.display(), capability.id);
        if self
            .capabilities
            .lock()
            .ok()
            .is_some_and(|capabilities| capabilities.contains(&key))
        {
            return Ok(());
        }

        executor.execute(CliInvocation {
            program: program.to_path_buf(),
            args: capability.args.clone(),
            runtime: ProcessContext::new(),
            stdio_mode: CliStdioMode::Capture,
        })?;

        match self.capabilities.lock() {
            Ok(mut capabilities) => {
                capabilities.insert(key);
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(key);
            }
        }

        Ok(())
    }
}
