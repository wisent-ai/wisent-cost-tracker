use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::{Value, json};
use std::path::Path;
use super::Run;

// An exact binary and decimal fraction that the former four-place rounding erased.
pub const COST_USD: f64 = 1.0 / 65_536.0;

pub fn configure(run: &Run, case: &str, central: bool) -> Result<()> {
    for (key, value) in run.environment(case, central)? { std::env::set_var(key, value); }
    Ok(())
}

pub fn json_value(py: Python<'_>, value: &Bound<'_, PyAny>) -> Result<Value> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("default", py.import("dataclasses")?.getattr("asdict")?)?;
    kwargs.set_item("allow_nan", false)?;
    let encoded: String = py.import("json")?.call_method("dumps", (value,), Some(&kwargs))?.extract()?;
    serde_json::from_str(&encoded).context("decode the SDK's actual JSON result")
}

pub fn object<'py>(py: Python<'py>, value: &Value) -> Result<Bound<'py, PyAny>> {
    Ok(py.import("json")?.call_method1("loads", (serde_json::to_string(value)?,))?)
}

pub fn uuid(py: Python<'_>) -> Result<String> {
    Ok(py.import("uuid")?.call_method0("uuid4")?.str()?.extract()?)
}

pub fn sha256(py: Python<'_>, path: &Path) -> Result<String> {
    let file = py.import("builtins")?.call_method1("open", (path, "rb"))?;
    let digest = py.import("hashlib")?.call_method1("file_digest", (&file, "sha256"));
    let closed = file.call_method0("close");
    let digest = digest.with_context(|| format!("hash {}", path.display()))?;
    closed?;
    Ok(digest.call_method0("hexdigest")?.extract()?)
}

pub fn installed_identity(py: Python<'_>, run: &Run) -> Result<Value> {
    let site = run.site().canonicalize()?;
    let suffixes: Vec<String> = py.import("importlib.machinery")?.getattr("EXTENSION_SUFFIXES")?.extract()?;
    let mut modules = Vec::new();
    for name in ["wisent_cost_tracker.spend", "wisent_cost_tracker.onboarding.engine.control_plane"] {
        let module = py.import(name)?;
        let file: std::path::PathBuf = module.getattr("__file__")?.extract()?;
        let file = file.canonicalize()?;
        ensure!(file.starts_with(&site), "{name} came from outside the just-installed candidate: {}", file.display());
        ensure!(suffixes.iter().any(|suffix| file.to_string_lossy().ends_with(suffix)),
            "{name} is not the native replacement: {}", file.display());
        modules.push(json!({"module": name, "file": file, "sha256": sha256(py, &file)?}));
    }
    let httpx = py.import("httpx")?;
    let httpx_file: std::path::PathBuf = httpx.getattr("__file__")?.extract()?;
    ensure!(httpx_file.canonicalize()?.starts_with(&site), "HTTPX did not come from the isolated install");
    let version: String = py.import("importlib.metadata")?.call_method1("version", ("wisent-cost-tracker",))?.extract()?;
    let interpreter: String = py.import("sys")?.getattr("version")?.extract()?;
    Ok(json!({"package_version": version, "python": interpreter, "native_modules": modules,
        "httpx": httpx.getattr("__version__")?.extract::<String>()?}))
}

pub fn tracker<'py>(py: Python<'py>, agent: &str, sink: &str,
    file: Option<&Path>, supabase: Option<(&str, &str)>, automatic: bool) -> Result<Bound<'py, PyAny>> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("agent_id", agent)?;
    kwargs.set_item("sink", sink)?;
    kwargs.set_item("auto_flush", automatic)?;
    if let Some(path) = file { kwargs.set_item("file_path", path)?; }
    if let Some((url, key)) = supabase {
        kwargs.set_item("supabase_url", url)?;
        kwargs.set_item("supabase_key", key)?;
    }
    Ok(py.import("wisent_cost_tracker")?.getattr("CostTracker")?.call((), Some(&kwargs))?)
}

pub fn record<'py>(py: Python<'py>, tracker: &Bound<'py, PyAny>, run_id: &str,
    marker: &str, cost: f64) -> Result<Bound<'py, PyAny>> {
    let metadata = PyDict::new(py);
    metadata.set_item("qualification_run", run_id)?;
    metadata.set_item("marker", marker)?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("service", "qualification_usage")?;
    kwargs.set_item("usage_type", "units")?;
    kwargs.set_item("usage_amount", 1)?;
    kwargs.set_item("cost_usd", cost)?;
    kwargs.set_item("reference_id", marker)?;
    kwargs.set_item("metadata", metadata)?;
    Ok(tracker.call_method("record", (), Some(&kwargs))?)
}

pub fn manager<'py>(py: Python<'py>, agent: &str, sink: &Bound<'py, PyAny>) -> Result<Bound<'py, PyAny>> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("sink", sink)?;
    Ok(py.import("wisent_cost_tracker")?.getattr("BudgetManager")?.call((agent,), Some(&kwargs))?)
}

pub fn since<'py>(py: Python<'py>) -> Result<Bound<'py, PyAny>> {
    let datetime = py.import("datetime")?;
    let utc = datetime.getattr("timezone")?.getattr("utc")?;
    Ok(datetime.getattr("datetime")?.call_method1("fromtimestamp", (0, utc))?)
}

pub fn budget_start<'py>(py: Python<'py>) -> Result<Bound<'py, PyAny>> {
    let datetime = py.import("datetime")?;
    let utc = datetime.getattr("timezone")?.getattr("utc")?;
    let now = datetime.getattr("datetime")?.call_method1("now", (utc,))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("days", 1)?;
    let previous_day = datetime.getattr("timedelta")?.call((), Some(&kwargs))?;
    Ok(now.call_method1("__sub__", (previous_day,))?)
}

pub fn record_json(py: Python<'_>, records: &Bound<'_, PyAny>) -> Result<Value> {
    let rows = PyList::empty(py);
    for record in records.try_iter()? { rows.append(record?.call_method0("to_json")?)?; }
    json_value(py, rows.as_any())
}
