use std::io::Write;
use std::path::{Path, PathBuf};
use pyo3::exceptions::{PyFileNotFoundError, PyOSError, PyValueError};
use pyo3::prelude::*;
use pyo3::gc::PyVisit;
use pyo3::PyTraverseError;
use pyo3::types::{PyDict, PyList, PyString};
use crate::bridge;

#[pyclass(weakref, module = "wisent_cost_tracker.spend.sinks")]
pub struct FileSink {
    #[pyo3(get, set)]
    path: Py<PyAny>,
}

#[pymethods]
impl FileSink {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.path)
    }

    fn __clear__(&mut self, py: Python<'_>) {
        self.path = py.None();
    }

    #[new]
    fn new(py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<Self> {
        let path = py.import("pathlib")?.getattr("Path")?.call1((path,))?;
        Ok(Self { path: path.unbind() })
    }

    fn write(&self, py: Python<'_>, records: &Bound<'_, PyAny>) -> PyResult<()> {
        let rows = self.rows(py)?;
        rows.call_method1("extend", (bridge::json_records(py, records)?,))?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("indent", 2)?;
        kwargs.set_item("allow_nan", false)?;
        let encoded = py.import("json")?.call_method("dumps", (rows,), Some(&kwargs))?;
        let contents = encoded.cast::<PyString>()?.to_str()?;
        let path: PathBuf = self.path.bind(py).extract()?;
        let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|error| io_error("create directory", parent, error))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| io_error("create replacement", &path, error))?;
        temporary.write_all(contents.as_bytes()).map_err(|error| io_error("write replacement", &path, error))?;
        temporary.as_file().sync_all().map_err(|error| io_error("sync replacement", &path, error))?;
        temporary.persist(&path).map_err(|error| io_error("replace", &path, error.error))?;
        Ok(())
    }

    fn read<'py>(&self, py: Python<'py>, agent_id: &str, since: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let rows = self.rows(py)?;
        let records = bridge::decoded_records(py, rows.as_any())?;
        let start = bridge::timestamp(py, since)?;
        let matched = PyList::empty(py);
        for record in records.iter() {
            if record.getattr("agent_id")?.eq(agent_id)? && bridge::record_timestamp(py, &record)? >= start {
                matched.append(record)?;
            }
        }
        Ok(matched)
    }
}

impl FileSink {
    fn rows<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let path = self.path.bind(py);
        let text = match path.call_method1("read_text", ("utf-8",)) {
            Ok(text) => text,
            Err(error) if error.is_instance_of::<PyFileNotFoundError>(py) => return Ok(PyList::empty(py)),
            Err(error) => return Err(error),
        };
        let decoded = py.import("json")?.call_method1("loads", (text,)).map_err(|cause| {
            let error = PyValueError::new_err(format!("FileSink cannot parse {path}: {cause}"));
            error.set_cause(py, Some(cause));
            error
        })?;
        decoded.cast_into::<PyList>().map_err(|cause| PyValueError::new_err(format!(
            "FileSink {path} must contain a JSON array: {cause}")))
    }
}

fn io_error(operation: &str, path: &Path, cause: std::io::Error) -> PyErr {
    PyOSError::new_err(format!("FileSink {operation} {} failed: {cause}", path.display()))
}
