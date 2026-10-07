//! Prices come from the packaged canonical table, never a second set of rates.

use std::path::PathBuf;

use pyo3::exceptions::{PyFileNotFoundError, PyOSError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;
const TOKENS_PER_PRICED_BLOCK: u64 = 1000;
const SECONDS_PER_HOUR: u64 = 3600;

fn read_pricing(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let package = py.import("wisent_cost_tracker")?;
    let here: PathBuf = package.getattr("__path__")?.get_item(0)?.extract()?;
    let here = here.canonicalize()?;
    let packaged = here.join("pricing/costs.json");
    let path = if packaged.try_exists()? {
        packaged
    } else {
        let mut found = None;
        for parent in here.ancestors().skip(1) {
            let candidate = parent.join("pricing/costs.json");
            if candidate.try_exists()? {
                found = Some(candidate);
                break;
            }
        }
        found.ok_or_else(|| PyFileNotFoundError::new_err("Could not locate pricing/costs.json"))?
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| PyOSError::new_err(format!("reading {}: {error}", path.display())))?;
    Ok(py
        .import("json")?
        .call_method1("loads", (text,))?
        .cast_into::<PyDict>()?)
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    let prices = read_pricing(module.py())?;
    module.add("_cached", &prices)?;
    module.add("PRICES", prices)?;
    module.add_function(wrap_pyfunction!(load_pricing, module)?)?;
    module.add_function(wrap_pyfunction!(captcha_price, module)?)?;
    module.add_function(wrap_pyfunction!(sms_price, module)?)?;
    module.add_function(wrap_pyfunction!(proxy_cost_for_bytes, module)?)?;
    module.add_function(wrap_pyfunction!(llm_cost, module)?)?;
    module.add_function(wrap_pyfunction!(compute_cost, module)?)?;
    Ok(())
}

#[pyfunction]
fn load_pricing(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let module = py.import("wisent_cost_tracker.pricing")?;
    let cached = module.getattr("_cached")?;
    if !cached.is_none() {
        return Ok(cached.cast_into::<PyDict>()?);
    }
    let prices = read_pricing(py)?;
    module.setattr("_cached", &prices)?;
    Ok(prices)
}

fn table<'py>(py: Python<'py>, section: &str) -> PyResult<Bound<'py, PyDict>> {
    Ok(py
        .import("wisent_cost_tracker.pricing")?
        .getattr("PRICES")?
        .get_item(section)?
        .cast_into::<PyDict>()?)
}

fn selected<'py>(prices: &Bound<'py, PyDict>, key: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
    match prices.get_item(key)? {
        Some(value) if !value.is_none() => Ok(Some(value)),
        _ => prices.get_item("default"),
    }
}

fn service_price(py: Python<'_>, section: &str, service: &str, resource: &str) -> PyResult<f64> {
    let services = table(py, section)?;
    if let Some(prices) = services.get_item(service)? {
        if let Some(price) = selected(prices.cast::<PyDict>()?, resource)? {
            if !price.is_none() {
                return price.extract();
            }
        }
    }
    Err(PyValueError::new_err(format!(
        "No declared {section} price for service={service:?}, resource={resource:?}; supply an explicit cost override"
    )))
}

#[pyfunction]
fn captcha_price(py: Python<'_>, service: &str, task_type: &str) -> PyResult<f64> {
    service_price(py, "captcha", service, task_type)
}

#[pyfunction]
fn sms_price(py: Python<'_>, service: &str, platform: &str) -> PyResult<f64> {
    service_price(py, "sms", service, &platform.to_lowercase())
}

fn fraction(py: Python<'_>, amount: &Bound<'_, PyAny>, divisor: u64) -> PyResult<f64> {
    // CPython divides arbitrary-sized integer counts before rounding to float.
    py.import("operator")?
        .call_method1("truediv", (amount, divisor))?
        .extract()
}

#[pyfunction]
#[pyo3(signature = (provider, num_bytes, is_mobile=false))]
fn proxy_cost_for_bytes(
    py: Python<'_>,
    provider: &str,
    num_bytes: &Bound<'_, PyAny>,
    is_mobile: bool,
) -> PyResult<f64> {
    let key = if is_mobile && provider == "oxylabs" {
        "oxylabs_mobile"
    } else {
        provider
    };
    let prices = table(py, "proxy_per_gb")?;
    let price = selected(&prices, key)?.ok_or_else(|| {
        PyValueError::new_err(format!(
            "No declared proxy price for provider={key:?} and no table default"
        ))
    })?;
    Ok(fraction(py, num_bytes, BYTES_PER_GIB)? * price.extract::<f64>()?)
}

#[pyfunction]
fn llm_cost(
    py: Python<'_>,
    model: &str,
    input_tokens: &Bound<'_, PyAny>,
    output_tokens: &Bound<'_, PyAny>,
) -> PyResult<f64> {
    let model = model.to_lowercase();
    let catalog = table(py, "llm")?;
    let mut matched = None;
    let mut match_length = 0;
    for (key, value) in catalog.iter() {
        let key = key.extract::<String>()?;
        if key == "default" {
            continue;
        }
        let normalized = key.to_lowercase();
        if normalized == model {
            matched = Some(value);
            break;
        }
        let length = normalized.encode_utf16().count();
        if length > match_length && model.contains(&normalized) {
            matched = Some(value);
            match_length = length;
        }
    }
    let prices = match matched {
        Some(prices) => prices,
        None => catalog.get_item("default")?.ok_or_else(|| {
            PyValueError::new_err(format!(
                "No declared LLM price for model={model:?} and no table default"
            ))
        })?,
    };
    Ok(
        fraction(py, input_tokens, TOKENS_PER_PRICED_BLOCK)? * prices.get_item("input_per_1k")?.extract::<f64>()?
            + fraction(py, output_tokens, TOKENS_PER_PRICED_BLOCK)?
                * prices.get_item("output_per_1k")?.extract::<f64>()?,
    )
}

#[pyfunction]
fn compute_cost(py: Python<'_>, instance_type: &str, seconds: &Bound<'_, PyAny>) -> PyResult<f64> {
    let prices = table(py, "compute_per_hour")?;
    let price = selected(&prices, instance_type)?.ok_or_else(|| {
        PyValueError::new_err(format!(
            "No declared compute price for instance_type={instance_type:?} and no table default"
        ))
    })?;
    Ok(fraction(py, seconds, SECONDS_PER_HOUR)? * price.extract::<f64>()?)
}
