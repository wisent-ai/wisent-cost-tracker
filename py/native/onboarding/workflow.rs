use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

pub(super) fn run<'py>(
    py: Python<'py>,
    budget: &Bound<'py, PyAny>,
    tokens: &Bound<'py, PyAny>,
    cost: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let tracker_module = py.import("wisent_cost_tracker.spend.tracker")?;
    let options = PyDict::new(py);
    options.set_item("agent_id", "first-use")?;
    options.set_item("auto_flush", false)?;
    let options = tracker_module.getattr("CostTrackerOptions")?.call((), Some(&options))?;
    let tracker = tracker_module.getattr("CostTracker")?.call1((options,))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("sink", tracker.call_method0("get_sink")?)?;
    let manager = py
        .import("wisent_cost_tracker.spend.budget")?
        .getattr("BudgetManager")?
        .call(("first-use",), Some(&kwargs))?;
    let now = py
        .import("datetime")?
        .getattr("datetime")?
        .call_method1("now", (crate::bridge::utc(py)?,))?;
    manager.call_method1("set_budget", ("all", budget, "daily", now))?;
    let baseline = manager.call_method1("get_status", ("all",))?;
    let usage = PyDict::new(py);
    usage.set_item("service", "llm_openai")?;
    usage.set_item("resource", "first-use-example")?;
    usage.set_item("usage_type", "tokens")?;
    usage.set_item("usage_amount", tokens)?;
    usage.set_item("cost_usd", cost)?;
    usage.set_item(
        "reference_id",
        format!("first-use-{}", py.import("uuid")?.call_method0("uuid4")?),
    )?;
    let metadata = PyDict::new(py);
    metadata.set_item("journey_id", super::definition(py)?.getattr("JOURNEY_ID")?)?;
    usage.set_item("metadata", metadata)?;
    let record = tracker.call_method("record", (), Some(&usage))?;
    tracker.call_method0("flush")?;
    let decision = manager.call_method1("decide", ("all",))?;
    let runtime = py
        .import("wisent_cost_tracker.onboarding.engine")?
        .getattr("Runtime")?
        .call0()?;
    runtime.call_method1("open", (false,))?;
    let statuses = PyList::empty(py);
    let status_json = py.import("wisent_cost_tracker.types")?.getattr("status_json")?;
    for status in baseline.try_iter()? {
        statuses.append(status_json.call1((status?,))?)?;
    }
    let result = PyDict::new(py);
    result.set_item("usage_accepted", true)?;
    result.set_item("usage", record.call_method0("to_json")?)?;
    result.set_item("baseline_budget_status", statuses)?;
    result.set_item("budget_decision", decision.call_method0("to_json")?)?;
    result.set_item("onboarding", runtime.call_method0("view")?)?;
    Ok(result.into_any())
}
