mod events;
#[path = "../native/transport.rs"]
mod transport;

use pyo3::exceptions::{PyException, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict};

fn definition(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    py.import("wisent_cost_tracker.onboarding.definition")
}

#[pyclass(module = "wisent_cost_tracker.onboarding.engine.control_plane")]
struct StadoTransport {
    #[pyo3(get, set)]
    base_url: String,
    #[pyo3(get, set)]
    token: String,
    #[pyo3(get, set)]
    available: bool,
    #[pyo3(get)]
    last_error: Option<String>,
}

#[pymethods]
impl StadoTransport {
    #[new]
    fn new(py: Python<'_>) -> PyResult<Self> {
        let environment = py.import("os")?.getattr("environ")?;
        let url: String = environment.call_method1("get", ("STADO_INTEGRATION_API_URL", ""))?.extract()?;
        let token = environment.call_method1("get", ("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN", ""))?.extract()?;
        Ok(Self { base_url: url.trim_end_matches('/').into(), token, available: true, last_error: None })
    }

    fn post<'py>(&mut self, py: Python<'py>, operation: &str, body: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        if !self.available {
            return Err(PyRuntimeError::new_err(self.last_error.clone().unwrap_or_else(|| "onboarding control plane is unavailable".into())));
        }
        if self.base_url.is_empty() || self.token.trim().is_empty() {
            return Err(PyRuntimeError::new_err("wisent-integrations requires its HTTPS origin in STADO_INTEGRATION_API_URL and the client bearer in WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN"));
        }
        let result = self.send(py, operation, body);
        match result {
            Ok(result) => Ok(result),
            Err(cause) if cause.is_instance_of::<PyException>(py) => {
                self.available = false;
                let detail = format!("wisent-integrations {operation} unavailable: {cause}");
                self.last_error = Some(detail.clone());
                eprintln!("{detail}");
                let error = PyRuntimeError::new_err(detail);
                error.set_cause(py, Some(cause));
                Err(error)
            }
            Err(cause) => Err(cause),
        }
    }

    fn read_bundle<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let body = identity(py)?;
        body.set_item("if_none_match", py.None())?;
        self.post(py, "bundle.read", body.as_any())
    }

    fn assign<'py>(&mut self, py: Python<'py>, subject_hash: &str) -> PyResult<Bound<'py, PyDict>> {
        let definition = definition(py)?;
        let body = PyDict::new(py);
        body.set_item("product_id", definition.getattr("PRODUCT_ID")?)?;
        body.set_item("app_id", definition.getattr("CLIENT_ID")?)?;
        body.set_item("subject", subject_hash)?;
        body.set_item("platform", "web")?;
        body.set_item("surface", "sdk_first_use")?;
        self.post(py, "experiments.assign", body.as_any())
    }

    fn read_state<'py>(&mut self, py: Python<'py>, progress: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        let definition = definition(py)?;
        let body = PyDict::new(py);
        body.set_item("product_id", definition.getattr("PRODUCT_ID")?)?;
        body.set_item("attempt_id", progress.get_item("attempt_id")?)?;
        body.set_item("subject_hash", progress.get_item("subject_hash")?)?;
        self.post(py, "state.read", body.as_any())
    }

    fn collect<'py>(&mut self, py: Python<'py>, event: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        self.post(py, "events.collect", event)
    }
}

impl StadoTransport {
    fn send<'py>(&self, py: Python<'py>, operation: &str, body: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        let parsed = py.import("urllib.parse")?.call_method1("urlparse", (&self.base_url,))?;
        let path: String = parsed.getattr("path")?.extract()?;
        if !parsed.getattr("scheme")?.eq("https")? || !parsed.getattr("netloc")?.is_truthy()?
            || parsed.getattr("username")?.is_truthy()? || parsed.getattr("password")?.is_truthy()?
            || parsed.getattr("query")?.is_truthy()? || parsed.getattr("fragment")?.is_truthy()?
            || parsed.getattr("params")?.is_truthy()? || (!path.is_empty() && path != "/") {
            return Err(PyRuntimeError::new_err("wisent-integrations URL must be an HTTPS origin without credentials, query, fragment or path"));
        }
        let payload = match body.cast::<PyDict>() {
            Ok(body) => body.clone(),
            Err(_) => {
                let payload = PyDict::new(py);
                payload.call_method1("update", (body,))?;
                payload
            }
        };
        let headers = PyDict::new(py);
        headers.set_item("Authorization", format!("Bearer {}", self.token))?;
        headers.set_item("Content-Type", "application/json")?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("headers", headers)?;
        kwargs.set_item("json", payload)?;
        let product: String = definition(py)?.getattr("PRODUCT_ID")?.extract()?;
        let url = format!("{}/api/integration/onboarding/{product}.{operation}", self.base_url);
        let response = transport::with_transport(py, &url, |transport| {
            transport::exchange(py, transport, operation, "POST", &url, &kwargs)
        })?;
        let decoded = transport::decode_json(py, &response, operation, "POST", &url)?;
        let envelope = decoded.cast::<PyDict>().map_err(|cause| PyRuntimeError::new_err(format!(
            "wisent-integrations {operation} returned a non-object envelope: {cause}; body: {decoded}")))?;
        let ok = envelope.get_item("ok")?.ok_or_else(|| PyRuntimeError::new_err(format!(
            "wisent-integrations {operation} returned an envelope without ok: {envelope}")))?;
        if !ok.is_instance_of::<PyBool>() || !ok.is_truthy()? {
            return Err(PyRuntimeError::new_err(format!("wisent-integrations {operation} refused the request: {envelope}")));
        }
        let result = envelope.get_item("result")?.ok_or_else(|| PyRuntimeError::new_err(format!(
            "wisent-integrations {operation} returned an envelope without result: {envelope}")))?;
        result.cast_into::<PyDict>().map_err(|cause| PyRuntimeError::new_err(format!(
            "wisent-integrations {operation} returned a non-object result: {cause}; envelope: {envelope}")))
    }
}

fn identity(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let definition = definition(py)?;
    let body = PyDict::new(py);
    for (field, constant) in [("product_id", "PRODUCT_ID"), ("journey_id", "JOURNEY_ID"), ("journey_version", "JOURNEY_VERSION")] {
        body.set_item(field, definition.getattr(constant)?)?;
    }
    Ok(body)
}

#[pymodule]
fn control_plane(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<StadoTransport>()?;
    module.add_function(wrap_pyfunction!(events::build_event, module)?)?;
    module.add_function(wrap_pyfunction!(events::flush, module)?)?;
    Ok(())
}
