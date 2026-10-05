use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use crate::{bridge, transport};

const RECORD_COLUMNS: &str = "service,usage_type,usage_amount,cost_usd,resource,reference_id,metadata,created_at,agent_id";
const STATUS_COLUMNS: &str = "category,allocated_usd,spent_usd,remaining_usd,utilization_pct,is_over_budget,period,starts_at";

#[pyclass(module = "wisent_cost_tracker.spend.sinks")]
pub struct SupabaseSink {
    #[pyo3(get, set)]
    url: String,
    #[pyo3(get, set)]
    key: String,
    #[pyo3(get, set)]
    table: String,
    #[pyo3(get, set)]
    budget_table: String,
    #[pyo3(get, set)]
    view_name: String,
}

#[pymethods]
impl SupabaseSink {
    #[new]
    #[pyo3(signature = (url, key, table="cost_records", budget_table="cost_budgets", view_name="cost_budget_status"))]
    fn new(py: Python<'_>, url: &str, key: String, table: &str, budget_table: &str, view_name: &str) -> PyResult<Self> {
        py.import("httpx")?;
        Ok(Self { url: url.trim_end_matches('/').into(), key, table: table.into(),
            budget_table: budget_table.into(), view_name: view_name.into() })
    }

    fn write(&self, py: Python<'_>, records: &Bound<'_, PyAny>) -> PyResult<()> {
        let rows = bridge::json_records(py, records)?;
        if !rows.is_empty() {
            transport::with_transport(py, &self.url, |transport| {
                self.request(py, transport, "write", "POST", &self.table, None,
                    Some(rows.as_any()), "return=minimal")
            })?;
        }
        Ok(())
    }

    fn read<'py>(&self, py: Python<'py>, agent_id: &str, since: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let params = PyDict::new(py);
        params.set_item("agent_id", format!("eq.{agent_id}"))?;
        let since: String = bridge::utc_datetime(py, since)?.call_method0("isoformat")?.extract()?;
        params.set_item("created_at", format!("gte.{since}"))?;
        params.set_item("select", RECORD_COLUMNS)?;
        params.set_item("order", "created_at.asc,id.asc")?;
        let rows = self.pages(py, "read", &self.table, &params)?;
        bridge::decoded_records(py, rows.as_any())
    }

    fn read_budgets<'py>(&self, py: Python<'py>, agent_id: &str) -> PyResult<Bound<'py, PyList>> {
        let params = PyDict::new(py);
        params.set_item("agent_id", format!("eq.{agent_id}"))?;
        params.set_item("select", STATUS_COLUMNS)?;
        params.set_item("order", "starts_at.asc,id.asc")?;
        let rows = self.pages(py, "read_budgets", &self.view_name, &params)?;
        let constructor = py.import("wisent_cost_tracker.types")?.getattr("BudgetStatus")?;
        let statuses = PyList::empty(py);
        for row in rows.iter() {
            statuses.append(constructor.call((), Some(row.cast::<PyDict>()?))?)?;
        }
        Ok(statuses)
    }

    fn write_budget(&self, py: Python<'_>, agent_id: &str, category: &str,
        allocated_usd: &Bound<'_, PyAny>, period: &str, starts_at: &Bound<'_, PyAny>) -> PyResult<()> {
        let params = PyDict::new(py);
        params.set_item("on_conflict", "agent_id,category,period,starts_at")?;
        let body = PyDict::new(py);
        body.set_item("agent_id", agent_id)?;
        body.set_item("category", category)?;
        body.set_item("allocated_usd", allocated_usd)?;
        body.set_item("period", period)?;
        body.set_item("starts_at", bridge::utc_datetime(py, starts_at)?.call_method0("isoformat")?)?;
        transport::with_transport(py, &self.url, |transport| {
            self.request(py, transport, "write_budget", "POST", &self.budget_table, Some(&params),
                Some(body.as_any()), "resolution=merge-duplicates,return=minimal")
        })?;
        Ok(())
    }
}

impl SupabaseSink {
    fn request<'py>(&self, py: Python<'py>, transport: &Bound<'py, PyAny>, operation: &str,
        method: &str, resource: &str, params: Option<&Bound<'py, PyDict>>,
        body: Option<&Bound<'py, PyAny>>, prefer: &str) -> PyResult<Bound<'py, PyAny>> {
        let url = format!("{}/rest/v1/{resource}", self.url);
        let headers = PyDict::new(py);
        headers.set_item("apikey", &self.key)?;
        headers.set_item("Authorization", format!("Bearer {}", self.key))?;
        headers.set_item("Content-Type", "application/json")?;
        headers.set_item("Prefer", prefer)?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("headers", headers)?;
        if let Some(params) = params { kwargs.set_item("params", params)?; }
        if let Some(body) = body { kwargs.set_item("json", body)?; }
        transport::exchange(py, transport, operation, method, &url, &kwargs)
    }

    fn pages<'py>(&self, py: Python<'py>, operation: &str, resource: &str,
        params: &Bound<'py, PyDict>) -> PyResult<Bound<'py, PyList>> {
        transport::with_transport(py, &self.url, |transport| {
            let rows = PyList::empty(py);
            loop {
                let offset = rows.len();
                params.set_item("offset", offset)?;
                let progress = format!("{operation} after {offset} rows");
                let response = self.request(py, transport, &progress, "GET", resource, Some(params),
                    None, "return=representation,count=exact")?;
                let url = format!("{}/rest/v1/{resource}", self.url);
                let decoded = transport::decode_json(py, &response, &progress, "GET", &url)?;
                let page = decoded.cast::<PyList>().map_err(|cause| PyRuntimeError::new_err(format!(
                    "SupabaseSink {progress} expected an array from {url}: {cause}; body: {decoded}")))?;
                let range: Option<String> = response.getattr("headers")?
                    .call_method1("get", ("content-range",))?.extract()?;
                let total = range.as_deref().and_then(|range| range.rsplit_once('/'))
                    .and_then(|(_, total)| total.parse::<usize>().ok());
                if page.is_empty() {
                    if total.is_some_and(|total| offset < total) {
                        return Err(PyRuntimeError::new_err(format!(
                            "SupabaseSink {operation} stopped after {offset} rows but Content-Range reports {range:?}")));
                    }
                    return Ok(rows);
                }
                rows.call_method1("extend", (page,))?;
                if total.is_some_and(|total| rows.len() >= total) { return Ok(rows); }
            }
        })
    }
}
