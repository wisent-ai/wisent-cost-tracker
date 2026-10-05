use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::gc::PyVisit;
use pyo3::PyTraverseError;
use pyo3::types::{PyDict, PyList, PyTuple};
use crate::bridge;

#[pyclass(weakref, module = "wisent_cost_tracker.spend.sinks")]
pub struct MemorySink {
    records: Option<Py<PyList>>,
    budgets: Py<PyDict>,
}

#[pymethods]
impl MemorySink {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.records)?;
        visit.call(&self.budgets)
    }

    fn __clear__(&mut self) {
        self.records = None;
    }

    #[getter(records)]
    fn get_records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        Ok(self.record_list(py)?.clone())
    }

    #[setter(records)]
    fn set_records(&mut self, records: Py<PyList>) {
        self.records = Some(records);
    }

    #[new]
    fn new(py: Python<'_>) -> Self {
        Self { records: Some(PyList::empty(py).unbind()), budgets: PyDict::new(py).unbind() }
    }

    fn write(&self, py: Python<'_>, records: &Bound<'_, PyAny>) -> PyResult<()> {
        self.record_list(py)?.call_method1("extend", (records,))?;
        Ok(())
    }

    fn read<'py>(&self, py: Python<'py>, agent_id: &str, since: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let start = bridge::timestamp(py, since)?;
        let rows = PyList::empty(py);
        for record in self.record_list(py)?.iter() {
            if record.getattr("agent_id")?.eq(agent_id)? && bridge::record_timestamp(py, &record)? >= start {
                rows.append(record)?;
            }
        }
        Ok(rows)
    }

    fn write_budget(&self, py: Python<'_>, agent_id: &str, category: &str,
        allocated_usd: &Bound<'_, PyAny>, period: &str, starts_at: &Bound<'_, PyAny>) -> PyResult<()> {
        period_days(period)?;
        self.budgets.bind(py).set_item(
            (agent_id, category, period, starts_at.call_method0("isoformat")?),
            (allocated_usd, starts_at))?;
        Ok(())
    }

    fn read_budgets<'py>(&self, py: Python<'py>, agent_id: &str) -> PyResult<Bound<'py, PyList>> {
        let rows = PyList::empty(py);
        let constructor = py.import("wisent_cost_tracker.types")?.getattr("BudgetStatus")?;
        let operators = py.import("operator")?;
        let add = operators.getattr("add")?;
        let subtract = operators.getattr("sub")?;
        let divide = operators.getattr("truediv")?;
        let multiply = operators.getattr("mul")?;
        for (key, value) in self.budgets.bind(py).iter() {
            let key = key.cast::<PyTuple>()?;
            if !key.get_item(0)?.eq(agent_id)? { continue; }
            let category = key.get_item(1)?;
            let period: String = key.get_item(2)?.extract()?;
            let value = value.cast::<PyTuple>()?;
            let allocated = value.get_item(0)?;
            let starts_at = value.get_item(1)?;
            let start = bridge::timestamp(py, &starts_at)?;
            let end = start + f64::from(period_days(&period)?) * 86400.0;
            let all = category.eq("all")?;
            let prefix = category.call_method1("__add__", ("_",))?;
            let mut spent = 0_i64.into_pyobject(py)?.into_any();
            for record in self.record_list(py)?.iter() {
                if !record.getattr("agent_id")?.eq(agent_id)? { continue; }
                let timestamp = bridge::record_timestamp(py, &record)?;
                if timestamp < start || timestamp >= end { continue; }
                if !all && !record.getattr("service")?.call_method1("startswith", (&prefix,))?.is_truthy()? { continue; }
                spent = add.call1((spent, record.getattr("cost_usd")?))?;
            }
            let kwargs = PyDict::new(py);
            kwargs.set_item("category", category)?;
            kwargs.set_item("allocated_usd", &allocated)?;
            kwargs.set_item("spent_usd", &spent)?;
            kwargs.set_item("remaining_usd", subtract.call1((&allocated, &spent))?)?;
            let utilization = if allocated.gt(0)? {
                multiply.call1((divide.call1((&spent, &allocated))?, 100))?
            } else { 0_i64.into_pyobject(py)?.into_any() };
            kwargs.set_item("utilization_pct", utilization)?;
            kwargs.set_item("is_over_budget", spent.gt(&allocated)?)?;
            kwargs.set_item("period", period)?;
            kwargs.set_item("starts_at", starts_at.call_method0("isoformat")?)?;
            rows.append(constructor.call((), Some(&kwargs))?)?;
        }
        Ok(rows)
    }
}

impl MemorySink {
    fn record_list<'py>(&self, py: Python<'py>) -> PyResult<&Bound<'py, PyList>> {
        self.records.as_ref().map(|records| records.bind(py))
            .ok_or_else(|| PyRuntimeError::new_err("MemorySink records were released by garbage collection"))
    }
}

fn period_days(period: &str) -> PyResult<u32> {
    // These are the fixed intervals in the canonical cost_budget_status view.
    match period {
        "daily" => Ok(1),
        "weekly" => Ok(7),
        "monthly" => Ok(30),
        _ => Err(PyValueError::new_err(format!("unsupported budget period {period:?}"))),
    }
}
