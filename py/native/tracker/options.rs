use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyList, PyString};

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = module.py();
    let string = py.get_type::<PyString>();
    let optional = py.import("typing")?.getattr("Optional")?;
    let optional_string = optional.get_item(&string)?;
    let optional_path = optional.get_item(py.import("pathlib")?.getattr("Path")?)?;
    let fields = PyList::empty(py);
    fields.append(("agent_id", &string))?;
    fields.append(("reference_id", &optional_string, py.None()))?;
    fields.append(("sink", &string, "memory"))?;
    fields.append(("file_path", optional_path, py.None()))?;
    fields.append(("supabase_url", &optional_string, py.None()))?;
    fields.append(("supabase_key", &optional_string, py.None()))?;
    fields.append(("auto_flush", py.get_type::<PyBool>(), true))?;
    let namespace = PyDict::new(py);
    namespace.set_item("__module__", "wisent_cost_tracker.spend.tracker")?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("namespace", namespace)?;
    let options = py.import("dataclasses")?.call_method("make_dataclass",
        ("CostTrackerOptions", fields), Some(&kwargs))?;
    options.setattr("__module__", "wisent_cost_tracker.spend.tracker")?;
    module.add("CostTrackerOptions", options)
}
