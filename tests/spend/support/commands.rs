use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use super::{Run, private_directory, write_json};

#[derive(Serialize, Deserialize)]
pub struct Outcome {
    pub label: String,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}

impl Outcome {
    pub fn success(&self) -> bool { self.code == Some(0) && self.signal.is_none() }

    pub fn require_success(&self) -> Result<()> {
        if self.success() { return Ok(()); }
        let stdout = std::fs::read_to_string(&self.stdout)?;
        let stderr = std::fs::read_to_string(&self.stderr)?;
        bail!("{} failed: exit {:?}, signal {:?}\nstdout:\n{}\nstderr:\n{}",
            self.label, self.code, self.signal, stdout, stderr)
    }
}

pub fn execute(run: &Run, label: &str, program: &Path, arguments: &[OsString],
    environment: &[(String, String)]) -> Result<Outcome> {
    ensure!(!label.is_empty() && label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "command evidence label is not a single filename");
    let directory = run.directory.join("commands");
    private_directory(&directory)?;
    let stdout = directory.join(format!("{label}.stdout"));
    let stderr = directory.join(format!("{label}.stderr"));
    write_json(&directory.join(format!("{label}.request.json")), &json!({
        "program": program,
        "arguments": arguments.iter().map(|v| v.to_string_lossy()).collect::<Vec<_>>(),
        "cwd": run.root,
        "environment_names": environment.iter().map(|(name, _)| name).collect::<Vec<_>>(),
    }))?;
    let output = OpenOptions::new().write(true).create_new(true).open(&stdout)?;
    let errors = OpenOptions::new().write(true).create_new(true).open(&stderr)?;
    let status = Command::new(program).args(arguments).current_dir(&run.root)
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null()).stdout(Stdio::from(output)).stderr(Stdio::from(errors)).status();
    let status = match status {
        Ok(status) => status,
        Err(cause) => {
            write_json(&directory.join(format!("{label}.exit.json")), &json!({"spawn_error": cause.to_string()}))?;
            return Err(cause).with_context(|| format!("start {label}: {}", program.display()));
        }
    };
    #[cfg(unix)]
    let signal = { use std::os::unix::process::ExitStatusExt; status.signal() };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let outcome = Outcome { label: label.into(), code: status.code(), signal, stdout, stderr };
    write_json(&directory.join(format!("{label}.exit.json")), &outcome)?;
    Ok(outcome)
}

pub fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

pub fn output(run: &Run, label: &str, program: &Path, arguments: &[OsString],
    environment: &[(String, String)]) -> Result<String> {
    let result = execute(run, label, program, arguments, environment)?;
    result.require_success()?;
    std::fs::read_to_string(&result.stdout).with_context(|| format!("read {label} output"))
}
