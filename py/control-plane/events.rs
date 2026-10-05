use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use super::definition;

#[pyfunction]
#[pyo3(signature = (name, *, progress, subject_hash, properties=None, screen_id=None, next_screen_id=None, reason_code=None))]
pub fn build_event<'py>(py: Python<'py>, name: &str, progress: &Bound<'py, PyAny>, subject_hash: &str,
    properties: Option<Bound<'py, PyAny>>, screen_id: Option<&str>, next_screen_id: Option<&str>,
    reason_code: Option<&str>) -> PyResult<Bound<'py, PyDict>> {
    let definition = definition(py)?;
    if !definition.getattr("CANONICAL_EVENTS")?.contains(name)? {
        return Err(PyValueError::new_err(format!("unsupported onboarding event {name}")));
    }
    let assignment = progress.call_method1("get", ("assignment", PyDict::new(py)))?;
    let event = PyDict::new(py);
    event.set_item("event_id", py.import("uuid")?.call_method0("uuid4")?.str()?)?;
    event.set_item("event_name", name)?;
    event.set_item("attempt_id", progress.get_item("attempt_id")?)?;
    for (field, constant) in [("product_id", "PRODUCT_ID"), ("journey_version_id", "JOURNEY_VERSION_ID")] {
        event.set_item(field, definition.getattr(constant)?)?;
    }
    event.set_item("subject_hash", subject_hash)?;
    event.set_item("scope_kind", "device")?;
    if let Some(screen) = screen_id.filter(|screen| !screen.is_empty()) { event.set_item("screen_id", screen)?; }
    else { event.set_item("screen_id", progress.get_item("current_screen_id")?)?; }
    event.set_item("occurred_at", definition.call_method0("now")?)?;
    event.set_item("evidence_revision", progress.get_item("evidence_revision")?)?;
    let experiment = assignment.call_method1("get", ("experiment_id",))?;
    if !experiment.is_none() {
        let variant = assignment.call_method1("get", ("variant_id",))?;
        if variant.is_none() {
            return Err(PyValueError::new_err("onboarding experiment assignment has no variant"));
        }
        event.set_item("experiment_id", experiment)?;
        event.set_item("variant_id", variant)?;
    }
    event.set_item("selected_next_screen_id", next_screen_id)?;
    event.set_item("reason_code", reason_code)?;
    let copied = PyDict::new(py);
    if let Some(properties) = properties {
        if properties.is_truthy()? { copied.call_method1("update", (properties,))?; }
    }
    event.set_item("properties", copied)?;
    event.set_item("answers", PyList::empty(py))?;
    Ok(event)
}

#[pyfunction]
pub fn flush(py: Python<'_>, pending: &Bound<'_, PyAny>, transport: &Bound<'_, PyAny>,
    persist: &Bound<'_, PyAny>) -> PyResult<()> {
    while pending.is_truthy()? {
        let event = pending.get_item(0)?;
        if migrate_queued_event(&event)? { persist.call0()?; }
        match transport.call_method1("collect", (event,)) {
            Ok(_) => (),
            Err(error) if error.is_instance_of::<PyRuntimeError>(py) => {
                eprintln!("Onboarding retained {} queued events: {error}", pending.len()?);
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        pending.call_method1("pop", (0,))?;
        persist.call0()?;
    }
    Ok(())
}

fn migrate_queued_event(event: &Bound<'_, PyAny>) -> PyResult<bool> {
    let event = event.cast::<PyDict>()?;
    let mut changed = false;
    // Rewrite the old queued wire shape once, retaining its identity and evidence.
    for field in ["journey_id", "journey_version"] {
        if event.contains(field)? {
            event.del_item(field)?;
            changed = true;
        }
    }
    if changed {
        let assigned = event.get_item("experiment_id")?.is_some_and(|id| !id.is_none());
        if !assigned {
            if let Some(variant) = event.get_item("variant_id")? {
                if variant.is_none() || variant.eq("control")? { event.del_item("variant_id")?; }
            }
            if event.contains("experiment_id")? { event.del_item("experiment_id")?; }
        }
    }
    Ok(changed)
}
