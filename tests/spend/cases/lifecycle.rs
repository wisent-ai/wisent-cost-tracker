use anyhow::{Result, bail, ensure};
use pyo3::exceptions::PyKeyboardInterrupt;
use pyo3::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::support::{Run, commands, private_directory, python, read_json, write_json};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode { Exit, Interrupt, Terminate }

impl Mode {
    fn name(self) -> Result<String> {
        Ok(serde_json::from_value(serde_json::to_value(self)?)?)
    }
}

pub fn worker(run: &Run, mode: Mode) -> Result<i32> {
    let name = mode.name()?;
    python::configure(run, &format!("lifecycle-{name}"), false)?;
    let directory = run.directory.join("lifecycle");
    private_directory(&directory)?;
    let file = directory.join(format!("{name}.records.json"));
    Python::initialize();
    // This child owns the main interpreter on its main thread. Keep its thread
    // state attached through finalization; no Python value may survive it.
    unsafe { pyo3::ffi::PyGILState_Ensure(); }
    let outcome = Python::attach(|py| -> Result<i32> {
        let identity = python::installed_identity(py, run)?;
        let agent = python::uuid(py)?;
        let tracker = python::tracker(py, &agent, "file", Some(&file), None, true)?;
        python::record(py, &tracker, &run.id, &name, python::COST_USD)?;
        let signal = py.import("signal")?;
        let number = match mode {
            Mode::Exit => None,
            Mode::Interrupt => Some(signal.getattr("SIGINT")?.extract::<i32>()?),
            Mode::Terminate => Some(signal.getattr("SIGTERM")?.extract::<i32>()?),
        };
        write_json(&directory.join(format!("{name}.ready.json")), &json!({
            "identity": identity, "agent": agent, "signal": number, "file": file,
        }))?;
        match number {
            None => Ok(0),
            Some(number) => match signal.call_method1("raise_signal", (number,)) {
                Err(error) if matches!(mode, Mode::Interrupt)
                    && error.is_instance_of::<PyKeyboardInterrupt>(py) => Ok(128 + number),
                Err(error) => Err(error.into()),
                Ok(_) => bail!("the real signal returned without the original exit behavior"),
            },
        }
    }).map_err(|error| format!("{error:#}"));
    // All Bound/Py/PyErr values above were dropped before this call. CPython
    // invokes registered atexit callbacks here, not a private callback runner.
    let finalized = unsafe { pyo3::ffi::Py_FinalizeEx() };
    ensure!(finalized == 0, "CPython finalization returned {finalized}");
    outcome.map_err(anyhow::Error::msg)
}

pub fn run(run: &Run) -> Result<Value> {
    let executable = std::env::current_exe()?;
    let mut observations = Vec::new();
    for mode in [Mode::Exit, Mode::Interrupt, Mode::Terminate] {
        let name = mode.name()?;
        let command = commands::execute(run, &format!("lifecycle-{name}"), &executable, &[
            "worker-lifecycle".into(), "--mode".into(), name.as_str().into(),
            "--run-dir".into(), run.directory.as_os_str().to_owned(),
        ], &[])?;
        let directory = run.directory.join("lifecycle");
        let ready: Value = read_json(&directory.join(format!("{name}.ready.json")))?;
        match mode {
            Mode::Exit => command.require_success()?,
            Mode::Interrupt => ensure!(command.signal.is_none()
                && command.code.map(i64::from) == ready["signal"].as_i64().map(|signal| 128 + signal),
                "SIGINT did not preserve KeyboardInterrupt exit: {}", serde_json::to_string(&command)?),
            Mode::Terminate => ensure!(command.code.is_none()
                && command.signal.map(i64::from) == ready["signal"].as_i64(),
                "SIGTERM did not preserve signal termination: {}", serde_json::to_string(&command)?),
        }
        let records: Value = read_json(&directory.join(format!("{name}.records.json")))?;
        ensure!(records.as_array().is_some_and(|rows| rows.len() == 1)
            && records[0]["agent_id"] == ready["agent"]
            && records[0]["reference_id"] == name
            && records[0]["cost_usd"].as_f64() == Some(python::COST_USD)
            && records[0]["metadata"]["qualification_run"] == run.id,
            "{name} did not persist precisely the pending owned record: {records}");
        observations.push(json!({"mode": mode, "exit": command, "ready": ready, "records": records}));
    }
    Ok(Value::Array(observations))
}
