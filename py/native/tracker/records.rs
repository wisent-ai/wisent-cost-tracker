use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use crate::bridge;
use super::CostTracker;

#[pymethods]
impl CostTracker {
    #[pyo3(signature = (service, usage_type, usage_amount, cost_usd, resource=None, reference_id=None, metadata=None))]
    fn record<'py>(&self, py: Python<'py>, service: &str, usage_type: &str,
        usage_amount: &Bound<'py, PyAny>, cost_usd: &Bound<'py, PyAny>, resource: Option<&str>,
        reference_id: Option<&str>, metadata: Option<&Bound<'py, PyDict>>) -> PyResult<Bound<'py, PyAny>> {
        if service.trim().is_empty() {
            return Err(PyValueError::new_err("CostTracker: service must be a non-empty string"));
        }
        // The usage types are the ones `wisent_cost_tracker.types.UsageType`
        // declares; this check reads that declaration rather than a copy.
        let declared = py
            .import("typing")?
            .call_method1("get_args", (py.import("wisent_cost_tracker.types")?.getattr("UsageType")?,))?;
        if !declared.contains(usage_type)? {
            return Err(PyValueError::new_err(format!("CostTracker: unsupported usage_type {usage_type:?}; declared: {declared}")));
        }
        bridge::numeric(py, usage_amount, "CostTracker: usage_amount", true)?;
        bridge::numeric(py, cost_usd, "CostTracker: cost_usd", false)?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("service", service)?;
        kwargs.set_item("resource", resource)?;
        kwargs.set_item("usage_type", usage_type)?;
        kwargs.set_item("usage_amount", py.get_type::<pyo3::types::PyFloat>().call1((usage_amount,))?)?;
        kwargs.set_item("cost_usd", cost_usd)?;
        if let Some(reference) = reference_id.filter(|value| !value.is_empty()) {
            kwargs.set_item("reference_id", reference)?;
        } else {
            kwargs.set_item("reference_id", self.options.bind(py).getattr("reference_id")?)?;
        }
        // A call that states no metadata records an empty mapping, the shape
        // CostRecord stores for "nothing stated".
        let metadata = match metadata.filter(|metadata| !metadata.is_empty()) {
            Some(metadata) => metadata.clone(),
            None => PyDict::new(py),
        };
        kwargs.set_item("metadata", metadata)?;
        kwargs.set_item("created_at", bridge::now_string(py)?)?;
        kwargs.set_item("agent_id", self.options.bind(py).getattr("agent_id")?)?;
        let record = bridge::record_type(py)?.call((), Some(&kwargs))?;
        self.buffer.bind(py).append(&record)?;
        Ok(record)
    }

    #[pyo3(signature = (service, task_type, r#override=None))]
    fn record_captcha<'py>(&self, py: Python<'py>, service: &str, task_type: &str,
        r#override: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let cost = match r#override {
            Some(cost) => cost,
            None => py.import("wisent_cost_tracker.pricing")?.call_method1("captcha_price", (service, task_type))?,
        };
        self.record(py, &format!("captcha_{service}"), "solves", one().bind(py), &cost, Some(task_type), None, None)
    }

    #[pyo3(signature = (provider, platform, r#override=None))]
    fn record_sms<'py>(&self, py: Python<'py>, provider: &str, platform: &str,
        r#override: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let cost = match r#override {
            Some(cost) => cost,
            None => py.import("wisent_cost_tracker.pricing")?.call_method1("sms_price", (provider, platform))?,
        };
        self.record(py, &format!("sms_{provider}"), "units", one().bind(py), &cost, Some(platform), None, None)
    }

    #[pyo3(signature = (provider, num_bytes, is_mobile=false))]
    fn record_proxy_bytes<'py>(&self, py: Python<'py>, provider: &str, num_bytes: &Bound<'py, PyAny>,
        is_mobile: bool) -> PyResult<Bound<'py, PyAny>> {
        let key = if is_mobile && provider == "oxylabs" { "oxylabs_mobile" } else { provider };
        let cost = py.import("wisent_cost_tracker.pricing")?
            .call_method1("proxy_cost_for_bytes", (provider, num_bytes, is_mobile))?;
        let resource = if is_mobile { "mobile" } else { "residential" };
        self.record(py, &format!("proxy_{key}"), "bytes", num_bytes, &cost, Some(resource), None, None)
    }

    #[pyo3(signature = (model, input_tokens=zero(), output_tokens=zero(), r#override=None, skill_id=None))]
    #[pyo3(text_signature = "($self, model, input_tokens=0, output_tokens=0, override=None, skill_id=None)")]
    fn record_llm<'py>(&self, py: Python<'py>, model: &str, input_tokens: Py<PyAny>,
        output_tokens: Py<PyAny>, r#override: Option<Bound<'py, PyAny>>, skill_id: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let input = input_tokens.bind(py);
        let output = output_tokens.bind(py);
        let metadata = PyDict::new(py);
        metadata.set_item("input_tokens", input)?;
        metadata.set_item("output_tokens", output)?;
        if let Some(skill) = skill_id.filter(|skill| !skill.is_empty()) { metadata.set_item("skill_id", skill)?; }
        let cost = match r#override {
            Some(cost) => cost,
            None => py.import("wisent_cost_tracker.pricing")?.call_method1("llm_cost", (model, input, output))?,
        };
        let amount = py.import("operator")?.call_method1("add", (input, output))?;
        // Filed under `llm` with the model as the resource: the provider is
        // not declared anywhere the tracker can read, so it is not guessed
        // from words in the model name.
        self.record(py, "llm", "tokens", &amount, &cost, Some(model), None, Some(&metadata))
    }

    #[pyo3(signature = (instance_type, seconds, r#override=None))]
    fn record_compute<'py>(&self, py: Python<'py>, instance_type: &str, seconds: &Bound<'py, PyAny>,
        r#override: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let cost = match r#override {
            Some(cost) => cost,
            None => py.import("wisent_cost_tracker.pricing")?.call_method1("compute_cost", (instance_type, seconds))?,
        };
        // Filed under `compute` with the instance type as the resource: an
        // undeclared instance type names no provider, so none is cut out of
        // its spelling.
        self.record(py, "compute", "seconds", seconds, &cost, Some(instance_type), None, None)
    }

    #[pyo3(signature = (provider, count=one(), r#override=None))]
    #[pyo3(text_signature = "($self, provider, count=1, override=None)")]
    fn record_email<'py>(&self, py: Python<'py>, provider: &str, count: Py<PyAny>,
        r#override: Option<Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let cost = match r#override {
            Some(cost) => cost,
            None => {
                let prices = py.import("wisent_cost_tracker.pricing")?.getattr("PRICES")?.get_item("email")?;
                let unit = prices.call_method1("get", (provider, prices.get_item("default")?))?;
                py.import("operator")?.call_method1("mul", (unit, count.bind(py)))?
            }
        };
        self.record(py, &format!("email_{provider}"), "emails", count.bind(py), &cost, Some(provider), None, None)
    }
}

/// The quantity a captcha, SMS or email helper records for one call
/// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/docs/record-helpers.md).
const SINGLE_EVENT_USAGE: u64 = 1;

/// The tokens a side of an LLM call that states none used, as `record_llm`'s
/// declared signature `input_tokens=0, output_tokens=0` says
/// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/README.md).
const OMITTED_TOKENS: u64 = 0;

fn zero() -> Py<PyAny> {
    Python::attach(|py| {
        let Ok(count) = OMITTED_TOKENS.into_pyobject(py);
        count.into_any().unbind()
    })
}

fn one() -> Py<PyAny> {
    Python::attach(|py| {
        let Ok(count) = SINGLE_EVENT_USAGE.into_pyobject(py);
        count.into_any().unbind()
    })
}
