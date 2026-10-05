use anyhow::{Result, ensure};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::{Value, json};
use std::path::Path;
use crate::support::{Run, commands, private_directory, python, read_json};
use python::COST_USD;

fn native(py: Python<'_>, run: &Run) -> Result<Value> {
    let agent = python::uuid(py)?;
    let tracker = python::tracker(py, &agent, "memory", None, None, false)?;
    let sink = tracker.call_method0("get_sink")?;
    let manager = python::manager(py, &agent, &sink)?;
    let starts = python::budget_start(py)?;
    manager.call_method1("set_budget", ("all", 2.0 * COST_USD, "weekly", &starts))?;
    let empty_decision = manager.call_method1("decide", ("all",));
    ensure!(empty_decision.is_err_and(|error| error.is_instance_of::<PyRuntimeError>(py)),
        "a budget without accepted usage must not produce a first-success decision");
    let first = python::record(py, &tracker, &run.id, "first", COST_USD)?;
    ensure!(first.getattr("cost_usd")?.extract::<f64>()? == COST_USD, "record quantized a valid small cost");
    tracker.call_method0("flush")?;
    let allowed = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(allowed["decision"] == "allow" && allowed["remaining_usd"].as_f64() == Some(COST_USD),
        "one accepted record must leave exactly one fixture cost available: {allowed}");
    python::record(py, &tracker, &run.id, "second", 2.0 * COST_USD)?;
    tracker.call_method0("flush")?;
    tracker.call_method0("flush")?;
    let snapshot = python::json_value(py, &tracker.call_method0("snapshot")?)?;
    ensure!(snapshot["cost_usd"].as_f64() == Some(3.0 * COST_USD), "snapshot lost small costs: {snapshot}");
    let persisted = python::record_json(py, &sink.call_method1("read", (&agent, python::since(py)?))?)?;
    ensure!(persisted.as_array().is_some_and(|rows| rows.len() == 2)
        && persisted[0]["reference_id"] == "first" && persisted[1]["reference_id"] == "second"
        && persisted[0]["cost_usd"].as_f64() == Some(COST_USD)
        && persisted[1]["cost_usd"].as_f64() == Some(2.0 * COST_USD),
        "later flushes lost or duplicated accepted usage: {persisted}");
    let denied = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(denied["decision"] == "deny" && denied["remaining_usd"].as_f64() == Some(-COST_USD)
        && denied["records_considered"] == 2, "overspending did not change the real decision: {denied}");
    manager.call_method1("set_budget", ("all", 4.0 * COST_USD, "weekly", &starts))?;
    let updated = python::json_value(py, &manager.call_method1("get_status", ("all",))?)?;
    ensure!(updated.as_array().is_some_and(|rows| rows.len() == 1)
        && updated[0]["allocated_usd"].as_f64() == Some(4.0 * COST_USD)
        && updated[0]["spent_usd"].as_f64() == Some(3.0 * COST_USD)
        && updated[0]["remaining_usd"].as_f64() == Some(COST_USD), "budget edit lost accepted spend: {updated}");
    let restored = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(restored["decision"] == "allow", "increased budget did not restore permission: {restored}");
    for refused in [
        tracker.call_method1("record", ("qualification_usage", "units", 0, COST_USD)),
        tracker.call_method1("record", ("qualification_usage", "units", 1, f64::INFINITY)),
        tracker.call_method1("record", ("qualification_usage", "units", true, COST_USD)),
    ] {
        ensure!(refused.is_err_and(|error| error.is_instance_of::<PyValueError>(py)), "invalid usage was accepted");
    }
    ensure!(python::json_value(py, &tracker.call_method0("snapshot")?)? == snapshot,
        "a refused record mutated the existing usage snapshot");
    let invalid_options = PyDict::new(py);
    invalid_options.set_item("agent_id", &agent)?;
    invalid_options.set_item("sink", "supabase")?;
    let refused = py.import("wisent_cost_tracker")?.getattr("CostTracker")?.call((), Some(&invalid_options));
    ensure!(refused.is_err_and(|error| error.is_instance_of::<PyValueError>(py)), "missing Supabase credentials did not refuse construction");

    let directory = run.directory.join("files");
    private_directory(&directory)?;
    let path = directory.join("native-records.json");
    let file_tracker = python::tracker(py, &agent, "file", Some(&path), None, false)?;
    python::record(py, &file_tracker, &run.id, "file-first", COST_USD)?;
    file_tracker.call_method0("flush")?;
    python::record(py, &file_tracker, &run.id, "file-second", 2.0 * COST_USD)?;
    file_tracker.call_method0("flush")?;
    file_tracker.call_method0("flush")?;
    let bytes = std::fs::read(&path)?;
    let file_rows: Value = serde_json::from_slice(&bytes)?;
    ensure!(file_rows.as_array().is_some_and(|rows| rows.len() == 2)
        && file_rows[0]["cost_usd"].as_f64() == Some(COST_USD)
        && file_rows[1]["cost_usd"].as_f64() == Some(2.0 * COST_USD), "actual file lost or duplicated usage: {file_rows}");
    let file_sink = file_tracker.call_method0("get_sink")?;
    let read_back = python::record_json(py, &file_sink.call_method1("read", (&agent, python::since(py)?))?)?;
    ensure!(read_back == file_rows, "FileSink did not read its real persisted records");
    let datetime = py.import("datetime")?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("tzinfo", datetime.getattr("timezone")?.getattr("utc")?)?;
    let after_records = datetime.getattr("datetime")?.getattr("max")?.call_method("replace", (), Some(&kwargs))?;
    let later = file_sink.call_method1("read", (&agent, after_records))?;
    ensure!(later.len()? == 0, "FileSink ignored the requested lower time boundary");
    ensure!(std::fs::read(&path)? == bytes, "reading a file mutated its persisted bytes");
    let corrupt_path = directory.join("native-corrupt.json");
    let corrupt = b"{unfinished-records\n";
    std::fs::write(&corrupt_path, corrupt)?;
    let corrupt_tracker = python::tracker(py, &agent, "file", Some(&corrupt_path), None, false)?;
    python::record(py, &corrupt_tracker, &run.id, "must-remain-pending", COST_USD)?;
    let refusal = corrupt_tracker.call_method0("flush");
    ensure!(refusal.is_err_and(|error| error.is_instance_of::<PyValueError>(py)), "corrupt storage was treated as an empty file");
    ensure!(std::fs::read(&corrupt_path)? == corrupt && corrupt_tracker.call_method0("total")?.extract::<f64>()? == COST_USD,
        "failed persistence discarded old bytes or pending usage");

    let weak = {
        let cyclic = python::tracker(py, &python::uuid(py)?, "memory", None, None, false)?;
        let record = python::record(py, &cyclic, &run.id, "cyclic-metadata", COST_USD)?;
        record.getattr("metadata")?.set_item("tracker", &cyclic)?;
        py.import("weakref")?.getattr("ref")?.call1((&cyclic,))?
    };
    py.import("gc")?.call_method0("collect")?;
    ensure!(weak.call0()?.is_none(), "native tracker retained an unreachable metadata cycle");
    Ok(json!({"allowed": allowed, "denied": denied, "updated": updated, "restored": restored,
        "snapshot": snapshot, "memory_records": persisted, "file": path, "file_records": file_rows,
        "corrupt_file_preserved": corrupt_path, "metadata_cycle_collected": true}))
}

pub fn run(run: &Run) -> Result<Value> {
    let native = Python::attach(|py| native(py, run))?;
    let priced_usage = Python::attach(|py| crate::pricing::run(py, run))?;
    let script = run.root.join("tests/spend/js/local.mjs");
    let command = commands::execute(run, "node-local", Path::new("node"), &[
        script.into_os_string(), run.directory.as_os_str().to_owned(),
    ], &run.environment("node-local", false)?)?;
    command.require_success()?;
    let node: Value = read_json(&command.stdout)?;
    Ok(json!({"native": native, "node": node, "priced_usage": priced_usage}))
}
