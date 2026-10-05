use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Write;
use super::{Run, private_directory, python, write_json};

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub range: Option<String>,
}

impl Reply {
    pub fn value(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).with_context(|| format!(
            "HTTP {} returned non-JSON: {}", self.status, String::from_utf8_lossy(&self.body)))
    }

    pub fn require_success(&self) -> Result<()> {
        ensure!((200..300).contains(&self.status), "HTTP {}: {}", self.status, String::from_utf8_lossy(&self.body));
        Ok(())
    }
}

pub struct Oracle<'a> {
    run: &'a Run,
    label: String,
    sequence: usize,
    base: String,
    key: String,
    supabase: bool,
}

#[derive(Serialize)]
struct RequestReceipt<'a> {
    operation: &'a str,
    method: &'a str,
    url: &'a str,
    body: Option<&'a Value>,
}

impl<'a> Oracle<'a> {
    pub fn new(run: &'a Run, label: &str, base: &str, key: String, supabase: bool) -> Self {
        Self { run, label: label.into(), sequence: 0, base: base.trim_end_matches('/').into(), key, supabase }
    }

    pub fn request(&mut self, py: Python<'_>, operation: &str, method: &str, path: &str,
        parameters: &[(&str, String)], body: Option<&Value>) -> Result<Reply> {
        let params = PyDict::new(py);
        for (name, value) in parameters { params.set_item(name, value)?; }
        let query: String = py.import("urllib.parse")?.call_method1("urlencode", (params,))?.extract()?;
        let url = if query.is_empty() { format!("{}{path}", self.base) }
            else { format!("{}{path}?{query}", self.base) };
        let directory = self.run.directory.join("http").join(&self.label);
        private_directory(&directory)?;
        let prefix = directory.join(self.sequence.to_string());
        self.sequence += 1;
        write_json(&prefix.with_extension("request.json"), &RequestReceipt { operation, method, url: &url, body })?;
        let headers = PyDict::new(py);
        headers.set_item("Authorization", format!("Bearer {}", self.key))?;
        headers.set_item("Content-Type", "application/json")?;
        headers.set_item("Prefer", "count=exact,return=representation")?;
        if self.supabase { headers.set_item("apikey", &self.key)?; }
        let kwargs = PyDict::new(py);
        kwargs.set_item("headers", headers)?;
        kwargs.set_item("method", method)?;
        if let Some(body) = body { kwargs.set_item("data", PyBytes::new(py, &serde_json::to_vec(body)?))?; }
        let urllib = py.import("urllib.request")?;
        let request = urllib.getattr("Request")?.call((&url,), Some(&kwargs))?;
        let http_error = py.import("urllib.error")?.getattr("HTTPError")?;
        let response = match urllib.call_method1("urlopen", (request,)) {
            Ok(response) => response,
            Err(error) if error.is_instance(py, &http_error) => error.value(py).clone().into_any(),
            Err(error) => {
                write_json(&prefix.with_extension("failure.json"), &json!({"operation": operation, "url": url, "error": error.to_string()}))?;
                return Err(error).with_context(|| format!("{operation}: {method} {url}"));
            }
        };
        let status: u16 = response.call_method0("getcode")?.extract()?;
        let received_headers = response.getattr("headers")?;
        let range: Option<String> = received_headers.call_method1("get", ("content-range",))?.extract()?;
        let header_pairs = python::json_value(py, &received_headers.call_method0("items")?)?;
        let received = response.call_method0("read");
        let closed = response.call_method0("close");
        let body: Vec<u8> = received.with_context(|| format!("read {operation}: HTTP {status} {url}"))?.extract()?;
        closed?;
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(prefix.with_extension("body"))?;
        output.write_all(&body)?;
        output.sync_all()?;
        write_json(&prefix.with_extension("response.json"), &json!({"status": status, "headers": header_pairs, "bytes": body.len()}))?;
        Ok(Reply { status, body, range })
    }

    pub fn rows(&mut self, py: Python<'_>, table: &str, agent: &str) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        loop {
            let reply = self.request(py, "inspect persisted rows", "GET", &format!("/rest/v1/{table}"), &[
                ("agent_id", format!("eq.{agent}")), ("select", "*".into()),
                ("order", "id.asc".into()), ("offset", rows.len().to_string()),
            ], None)?;
            reply.require_success()?;
            let value = reply.value()?;
            let Value::Array(page) = value else { anyhow::bail!("persisted {table} response is not an array"); };
            let total = reply.range.as_deref().and_then(|r| r.rsplit_once('/'))
                .and_then(|(_, total)| total.parse::<usize>().ok());
            if page.is_empty() {
                ensure!(total.is_none_or(|total| rows.len() == total), "persisted {table} stopped before its declared total");
                return Ok(rows);
            }
            rows.extend(page);
            if total.is_some_and(|total| rows.len() >= total) { return Ok(rows); }
        }
    }
}
