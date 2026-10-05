mod lifecycle;
mod options;
mod records;

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::gc::PyVisit;
use pyo3::PyTraverseError;
use pyo3::types::{PyDict, PyList, PyTuple};
use crate::bridge;

#[pyclass(weakref, module = "wisent_cost_tracker.spend.tracker")]
pub struct CostTracker {
    options: Py<PyAny>,
    buffer: Py<PyList>,
    sink: Py<PyAny>,
    accepted: AtomicUsize,
    flushing: AtomicBool,
    pending_signal: AtomicI32,
    previous_signals: Py<PyDict>,
}

#[pymethods]
impl CostTracker {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.options)?;
        visit.call(&self.buffer)?;
        visit.call(&self.sink)?;
        visit.call(&self.previous_signals)
    }

    #[new]
    #[pyo3(signature = (opts=None, **kwargs))]
    fn new(py: Python<'_>, opts: Option<Py<PyAny>>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<Py<Self>> {
        let options = match opts {
            Some(options) => options,
            None => py.import("wisent_cost_tracker.spend.tracker")?.getattr("CostTrackerOptions")?
                .call((), kwargs)?.unbind(),
        };
        let options_ref = options.bind(py);
        let agent = options_ref.getattr("agent_id")?;
        let agent: String = agent.extract().map_err(|_| PyValueError::new_err("CostTracker: agent_id must be a non-empty string"))?;
        if agent.trim().is_empty() {
            return Err(PyValueError::new_err("CostTracker: agent_id must be a non-empty string"));
        }
        let sinks = py.import("wisent_cost_tracker.spend.sinks")?;
        let kind: String = options_ref.getattr("sink")?.extract()?;
        let sink = match kind.as_str() {
            "supabase" => {
                let url = options_ref.getattr("supabase_url")?;
                let key = options_ref.getattr("supabase_key")?;
                if !url.is_truthy()? || !key.is_truthy()? {
                    return Err(PyValueError::new_err("CostTracker: sink='supabase' requires supabase_url + supabase_key"));
                }
                sinks.getattr("SupabaseSink")?.call1((url, key))?
            }
            "file" => {
                let path = options_ref.getattr("file_path")?;
                if !path.is_truthy()? { return Err(PyValueError::new_err("CostTracker: sink='file' requires file_path")); }
                sinks.getattr("FileSink")?.call1((path,))?
            }
            "memory" => sinks.getattr("MemorySink")?.call0()?,
            _ => return Err(PyValueError::new_err(format!("CostTracker: unsupported sink {kind:?}"))),
        };
        let automatic = options_ref.getattr("auto_flush")?.is_truthy()?;
        let tracker = Py::new(py, Self {
            options, buffer: PyList::empty(py).unbind(), sink: sink.unbind(),
            accepted: AtomicUsize::new(0), flushing: AtomicBool::new(false),
            pending_signal: AtomicI32::new(0), previous_signals: PyDict::new(py).unbind(),
        })?;
        if automatic { lifecycle::register(py, &tracker)?; }
        Ok(tracker)
    }

    fn total<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        bridge::total(py, self.buffer.bind(py))
    }

    fn snapshot<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let service_costs = PyDict::new(py);
        let add = py.import("operator")?.getattr("add")?;
        for record in self.buffer.bind(py).iter() {
            let service = record.getattr("service")?;
            let previous = match service_costs.get_item(&service)? {
                Some(value) => value,
                None => 0_i64.into_pyobject(py)?.into_any(),
            };
            service_costs.set_item(service, add.call1((previous, record.getattr("cost_usd")?))?)?;
        }
        let snapshot = PyDict::new(py);
        snapshot.set_item("cost_usd", self.total(py)?)?;
        snapshot.set_item("service_costs", service_costs)?;
        snapshot.set_item("records", bridge::json_records(py, self.buffer.bind(py).as_any())?)?;
        Ok(snapshot)
    }

    pub(super) fn flush(&self, py: Python<'_>) -> PyResult<()> {
        if self.flushing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return Err(PyRuntimeError::new_err("CostTracker: a flush is already in progress; pending records were not resent"));
        }
        let start = self.accepted.load(Ordering::Relaxed);
        let result: PyResult<()> = (|| {
            let buffer = self.buffer.bind(py);
            let end = buffer.len();
            if start >= end { return Ok(()); }
            let pending = buffer.get_slice(start, end);
            self.sink.bind(py).call_method1("write", (&pending,))?;
            self.accepted.store(end, Ordering::Relaxed);
            bridge::observe(py, "observe_accepted_usage", &PyTuple::new(py, [pending])?)?;
            Ok(())
        })();
        self.flushing.store(false, Ordering::Release);
        let pending_signal = self.pending_signal.swap(0, Ordering::AcqRel);
        if pending_signal != 0 {
            let accepted = self.accepted.load(Ordering::Relaxed);
            if (result.is_ok() || accepted > start) && accepted < self.buffer.bind(py).len() {
                if let Err(error) = self.flush(py) { error.print(py); }
            }
            if let Err(error) = &result { error.print(py); }
            self.forward_signal(py, pending_signal, py.None().bind(py))?;
        }
        result
    }

    fn get_sink<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.sink.bind(py).clone()
    }
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    options::register(module)?;
    module.add_class::<CostTracker>()
}
