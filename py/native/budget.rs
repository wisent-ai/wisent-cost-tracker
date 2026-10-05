use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::gc::PyVisit;
use pyo3::PyTraverseError;
use pyo3::types::{PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use crate::bridge;

#[pyclass(weakref, module = "wisent_cost_tracker.spend.budget")]
pub struct BudgetManager {
    #[pyo3(get, set)]
    agent_id: String,
    #[pyo3(get, set)]
    sink: Py<PyAny>,
}

#[pymethods]
impl BudgetManager {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.sink)
    }

    fn __clear__(&mut self, py: Python<'_>) {
        self.sink = py.None();
    }

    #[new]
    #[pyo3(signature = (agent_id, sink=None, supabase_url=None, supabase_key=None))]
    fn new(py: Python<'_>, agent_id: String, sink: Option<Py<PyAny>>,
        supabase_url: Option<&str>, supabase_key: Option<&str>) -> PyResult<Self> {
        if agent_id.trim().is_empty() {
            return Err(PyValueError::new_err("BudgetManager: agent_id must be a non-empty string"));
        }
        let sink = match sink {
            Some(sink) => sink,
            None => match (supabase_url.filter(|url| !url.is_empty()), supabase_key.filter(|key| !key.is_empty())) {
                (Some(url), Some(key)) => py.import("wisent_cost_tracker.spend.sinks")?
                    .getattr("SupabaseSink")?.call1((url, key))?.unbind(),
                _ => return Err(PyValueError::new_err("BudgetManager: provide either sink= or supabase_url + supabase_key")),
            },
        };
        Ok(Self { agent_id, sink })
    }

    #[pyo3(signature = (category, allocated_usd, period, starts_at=None))]
    fn set_budget(&self, py: Python<'_>, category: &str, allocated_usd: &Bound<'_, PyAny>,
        period: &str, starts_at: Option<Bound<'_, PyAny>>) -> PyResult<()> {
        if category.is_empty() { return Err(PyValueError::new_err("BudgetManager: category must be a non-empty string")); }
        bridge::numeric(py, allocated_usd, "BudgetManager: allocated_usd", false)?;
        if !matches!(period, "daily" | "weekly" | "monthly") {
            return Err(PyValueError::new_err(format!("BudgetManager: unsupported period {period:?}")));
        }
        let sink = self.sink.bind(py);
        if !sink.hasattr("write_budget")? {
            return Err(PyRuntimeError::new_err("BudgetManager: sink does not support write_budget"));
        }
        let starts_at = match starts_at {
            Some(starts_at) => starts_at,
            None => py.import("datetime")?.getattr("datetime")?.call_method1("now", (bridge::utc(py)?,))?,
        };
        sink.call_method1("write_budget", (&self.agent_id, category, allocated_usd, period, starts_at))?;
        Ok(())
    }

    #[pyo3(signature = (category=None))]
    fn get_status<'py>(&self, py: Python<'py>, category: Option<&str>) -> PyResult<Bound<'py, PyList>> {
        let sink = self.sink.bind(py);
        if !sink.hasattr("read_budgets")? {
            return Err(PyRuntimeError::new_err("BudgetManager: sink does not support read_budgets"));
        }
        let rows = PyList::empty(py);
        for row in sink.call_method1("read_budgets", (&self.agent_id,))?.try_iter()? {
            let row = row?;
            if category.is_none() || row.getattr("category")?.eq(category)? { rows.append(row)?; }
        }
        if !rows.is_empty() {
            bridge::observe(py, "observe_budget_status", &PyTuple::new(py, [&rows])?)?;
        }
        Ok(rows)
    }

    fn is_over_budget(&self, py: Python<'_>, category: &str) -> PyResult<bool> {
        for status in self.get_status(py, Some(category))?.iter() {
            if status.getattr("is_over_budget")?.is_truthy()? { return Ok(true); }
        }
        Ok(false)
    }

    fn remaining<'py>(&self, py: Python<'py>, category: &str) -> PyResult<Bound<'py, PyAny>> {
        let rows = self.get_status(py, Some(category))?;
        if rows.is_empty() { return Ok(f64::INFINITY.into_pyobject(py)?.into_any()); }
        remaining_total(py, &rows)
    }

    fn decide<'py>(&self, py: Python<'py>, category: &str) -> PyResult<Bound<'py, PyAny>> {
        if category.is_empty() { return Err(PyValueError::new_err("BudgetManager: category must be a non-empty string")); }
        let statuses = self.get_status(py, Some(category))?;
        if statuses.is_empty() {
            return Err(PyRuntimeError::new_err("BudgetManager: no budget is configured for this category"));
        }
        let sink = self.sink.bind(py);
        if !sink.hasattr("read")? {
            return Err(PyRuntimeError::new_err("BudgetManager: sink cannot confirm accepted usage"));
        }
        let datetime = py.import("datetime")?.getattr("datetime")?;
        let first = statuses.get_item(0)?.getattr("starts_at")?;
        let mut start = bridge::utc_datetime(py, &datetime.call_method1("fromisoformat", (first,))?)?;
        let mut over = false;
        for status in statuses.iter() {
            let parsed = datetime.call_method1("fromisoformat", (status.getattr("starts_at")?,))?;
            let parsed = bridge::utc_datetime(py, &parsed)?;
            if parsed.lt(&start)? { start = parsed; }
            over |= status.getattr("is_over_budget")?.is_truthy()?;
        }
        let matching = PyList::empty(py);
        let record_type = bridge::record_type(py)?;
        let prefix = format!("{category}_").into_pyobject(py)?;
        for record in sink.call_method1("read", (&self.agent_id, start))?.try_iter()? {
            let record = record?;
            if !valid_record(py, &record, &record_type, &self.agent_id)? { continue; }
            if category == "all" || record.getattr("service")?.call_method1("startswith", (&prefix,))?.is_truthy()? {
                matching.append(record)?;
            }
        }
        if matching.is_empty() {
            return Err(PyRuntimeError::new_err("BudgetManager: no accepted usage exists for this budget decision"));
        }
        let kwargs = PyDict::new(py);
        kwargs.set_item("category", category)?;
        kwargs.set_item("decision", if over { "deny" } else { "allow" })?;
        kwargs.set_item("is_over_budget", over)?;
        kwargs.set_item("remaining_usd", remaining_total(py, &statuses)?)?;
        kwargs.set_item("records_considered", matching.len())?;
        kwargs.set_item("statuses", statuses)?;
        let decision = py.import("wisent_cost_tracker.types")?.getattr("BudgetDecision")?.call((), Some(&kwargs))?;
        for index in (0..matching.len()).rev() {
            let args = PyTuple::new(py, [matching.get_item(index)?, decision.clone()])?;
            if let Some(observed) = bridge::observe(py, "observe_budget_decision", &args)? {
                if observed.bind(py).is_truthy()? { break; }
            }
        }
        Ok(decision)
    }
}

fn remaining_total<'py>(py: Python<'py>, statuses: &Bound<'py, PyList>) -> PyResult<Bound<'py, PyAny>> {
    let builtins = py.import("builtins")?;
    let getter = py.import("operator")?.getattr("attrgetter")?.call1(("remaining_usd",))?;
    let amounts = builtins.getattr("map")?.call1((getter, statuses))?;
    builtins.call_method1("sum", (amounts,))
}

fn valid_record(py: Python<'_>, record: &Bound<'_, PyAny>, constructor: &Bound<'_, PyAny>, agent: &str) -> PyResult<bool> {
    if !record.is_instance(constructor)? || !record.getattr("agent_id")?.eq(agent)? { return Ok(false); }
    for name in ["service", "created_at"] {
        let value = record.getattr(name)?;
        if !value.is_instance_of::<PyString>() || !value.is_truthy()? { return Ok(false); }
    }
    let math = py.import("math")?;
    for name in ["usage_amount", "cost_usd"] {
        let value = record.getattr(name)?;
        if !(value.get_type().is(&py.get_type::<PyInt>()) || value.get_type().is(&py.get_type::<PyFloat>()))
            || !math.call_method1("isfinite", (value,))?.is_truthy()? { return Ok(false); }
    }
    Ok(true)
}
