use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFloat, PyInt, PyList};

pub fn record_type(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    py.import("wisent_cost_tracker.types")?.getattr("CostRecord")
}

pub fn json_records<'py>(py: Python<'py>, records: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let rows = PyList::empty(py);
    for record in records.try_iter()? {
        rows.append(record?.call_method0("to_json")?)?;
    }
    Ok(rows)
}

pub fn decoded_records<'py>(py: Python<'py>, rows: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let constructor = record_type(py)?;
    let records = PyList::empty(py);
    for row in rows.cast::<PyList>()?.iter() {
        records.append(constructor.call((), Some(row.cast::<PyDict>()?))?)?;
    }
    Ok(records)
}

pub fn utc<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    py.import("datetime")?.getattr("timezone")?.getattr("utc")
}

pub fn utc_datetime<'py>(py: Python<'py>, value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    if value.getattr("tzinfo")?.is_none() {
        let kwargs = PyDict::new(py);
        kwargs.set_item("tzinfo", utc(py)?)?;
        value.call_method("replace", (), Some(&kwargs))
    } else {
        value.call_method1("astimezone", (utc(py)?,))
    }
}

pub fn timestamp(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<f64> {
    utc_datetime(py, value)?.call_method0("timestamp")?.extract()
}

pub fn record_timestamp(py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<f64> {
    let created = record.getattr("created_at")?;
    if created.is_none() || !created.is_truthy()? {
        return Ok(0.0);
    }
    let parsed = py.import("datetime")?.getattr("datetime")?
        .call_method1("fromisoformat", (created,));
    match parsed {
        Ok(value) => timestamp(py, &value),
        Err(error) if error.is_instance_of::<PyValueError>(py) => Ok(0.0),
        Err(error) => Err(error),
    }
}

pub fn now_string(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let value = py.import("datetime")?.getattr("datetime")?
        .call_method1("now", (utc(py)?,))?.call_method0("isoformat")?;
    value.call_method1("replace", ("+00:00", "Z"))
}

pub fn numeric(py: Python<'_>, value: &Bound<'_, PyAny>, name: &str, positive: bool) -> PyResult<()> {
    let numeric_type = value.get_type().is(&py.get_type::<PyInt>())
        || value.get_type().is(&py.get_type::<PyFloat>());
    let finite = numeric_type && py.import("math")?.call_method1("isfinite", (value,))?.is_truthy()?;
    let valid_sign = finite && if positive { value.gt(0)? } else { value.ge(0)? };
    if !valid_sign {
        let constraint = if positive { "greater than zero" } else { "non-negative" };
        return Err(PyValueError::new_err(format!("{name} must be a finite number {constraint}")));
    }
    Ok(())
}

pub fn total<'py>(py: Python<'py>, records: &Bound<'py, PyList>) -> PyResult<Bound<'py, PyAny>> {
    let builtins = py.import("builtins")?;
    let attribute = py.import("operator")?.getattr("attrgetter")?.call1(("cost_usd",))?;
    let costs = builtins.getattr("map")?.call1((attribute, records))?;
    builtins.call_method1("sum", (costs,))
}

pub fn observe(py: Python<'_>, operation: &str, args: &Bound<'_, pyo3::types::PyTuple>) -> PyResult<Option<Py<PyAny>>> {
    let result = py.import("wisent_cost_tracker.onboarding")?
        .getattr(operation)?.call1(args);
    match result {
        Ok(value) => Ok(Some(value.unbind())),
        Err(error) if error.is_instance_of::<pyo3::exceptions::PyOSError>(py)
            || error.is_instance_of::<PyTypeError>(py)
            || error.is_instance_of::<PyValueError>(py) => {
                eprintln!("Cost tracker onboarding observation {operation} failed: {error}");
                error.print(py);
                Ok(None)
            }
        Err(error) => Err(error),
    }
}
