use pyo3::exceptions::{PyException, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

pub fn with_transport<'py, T>(py: Python<'py>, url: &str,
    run: impl FnOnce(&Bound<'py, PyAny>) -> PyResult<T>) -> PyResult<T> {
    let httpx = py.import("httpx")?;
    let parsed = httpx.getattr("URL")?.call1((url,))?;
    let urllib = py.import("urllib.request")?;
    let host = parsed.getattr("netloc")?.call_method0("decode")?;
    let kwargs = PyDict::new(py);
    if !urllib.call_method1("proxy_bypass", (host,))?.is_truthy()? {
        let proxies = urllib.call_method0("getproxies")?;
        let scheme = parsed.getattr("scheme")?;
        let mut proxy = proxies.call_method1("get", (scheme,))?;
        if proxy.is_none() { proxy = proxies.call_method1("get", ("all",))?; }
        if !proxy.is_none() {
            let proxy: String = proxy.extract()?;
            let proxy = if proxy.contains("://") { proxy } else { format!("http://{proxy}") };
            kwargs.set_item("proxy", proxy)?;
        }
    }
    let transport = httpx.getattr("HTTPTransport")?.call((), Some(&kwargs))?;
    let result = run(&transport);
    if let Err(cause) = transport.call_method0("close") {
        let error = PyRuntimeError::new_err(format!("HTTP transport close for {url} failed: {cause}"));
        error.set_cause(py, Some(cause));
        error.print(py);
    }
    result
}

pub fn exchange<'py>(py: Python<'py>, transport: &Bound<'py, PyAny>, operation: &str,
    method: &str, url: &str, kwargs: &Bound<'py, PyDict>) -> PyResult<Bound<'py, PyAny>> {
    let request = py.import("httpx")?.getattr("Request")?.call((method, url), Some(kwargs))?;
    let result = (|| {
        let response = transport.call_method1("handle_request", (request,))?;
        response.call_method0("read")?;
        Ok(response)
    })();
    let response: Bound<'py, PyAny> = result.map_err(|cause: PyErr| {
        if !cause.is_instance_of::<PyException>(py) { return cause; }
        let error = PyRuntimeError::new_err(format!("HTTP {operation} {method} {url} failed: {cause}"));
        error.set_cause(py, Some(cause));
        error
    })?;
    let status: u16 = response.getattr("status_code")?.extract()?;
    if !(200..300).contains(&status) {
        let body = response.getattr("text")?;
        let error = PyRuntimeError::new_err(format!("HTTP {operation} {method} {url} failed: {status} {body}"));
        let details = error.value(py);
        details.setattr("operation", operation)?;
        details.setattr("method", method)?;
        details.setattr("url", url)?;
        details.setattr("status_code", status)?;
        details.setattr("response_body", body)?;
        return Err(error);
    }
    Ok(response)
}

pub fn decode_json<'py>(py: Python<'py>, response: &Bound<'py, PyAny>,
    operation: &str, method: &str, url: &str) -> PyResult<Bound<'py, PyAny>> {
    match response.call_method0("json") {
        Ok(value) => Ok(value),
        Err(cause) if cause.is_instance_of::<PyException>(py) => {
            let body = response.getattr("text")?;
            let status = response.getattr("status_code")?;
            let error = PyRuntimeError::new_err(format!(
                "HTTP {operation} {method} {url} returned invalid JSON: {cause}; status: {status}; body: {body}"));
            let details = error.value(py);
            details.setattr("operation", operation)?;
            details.setattr("method", method)?;
            details.setattr("url", url)?;
            details.setattr("status_code", status)?;
            details.setattr("response_body", body)?;
            error.set_cause(py, Some(cause));
            Err(error)
        }
        Err(cause) => Err(cause),
    }
}
