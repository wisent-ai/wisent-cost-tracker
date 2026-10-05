use anyhow::{Result, ensure};
use pyo3::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use super::{Run, credential, private_directory, python, read_json, write_json};
use super::oracle::Oracle;

#[derive(Serialize, Deserialize)]
pub struct OwnedAgent {
    pub agent: String,
    pub run_id: String,
    pub client: String,
    pub starts_at: String,
}

pub fn reserve(py: Python<'_>, run: &Run, client: &str, oracle: &mut Oracle<'_>) -> Result<OwnedAgent> {
    let agent = format!("qualification-{}-{}", run.id, python::uuid(py)?);
    ensure!(oracle.rows(py, "cost_records", &agent)?.is_empty(), "generated agent already has records; no ownership was claimed");
    ensure!(oracle.rows(py, "cost_budgets", &agent)?.is_empty(), "generated agent already has budgets; no ownership was claimed");
    let starts = python::budget_start(py)?;
    let kwargs = pyo3::types::PyDict::new(py);
    kwargs.set_item("microsecond", 0)?;
    let starts_at = starts.call_method("replace", (), Some(&kwargs))?.call_method0("isoformat")?.extract()?;
    let owned = OwnedAgent { agent, run_id: run.id.clone(), client: client.into(), starts_at };
    write_json(&run.directory.join("owned").join(format!("{client}.json")), &owned)?;
    Ok(owned)
}

pub fn category(run: &Run, index: usize) -> String {
    format!("qualification-{}-budget-{index}", run.id)
}

pub fn cleanup(py: Python<'_>, run: &Run, owned: &OwnedAgent, oracle: &mut Oracle<'_>) -> Result<Value> {
    ensure!(owned.run_id == run.id, "owned agent receipt names another run");
    let records = oracle.rows(py, "cost_records", &owned.agent)?;
    for record in &records {
        ensure!(record.pointer("/metadata/qualification_run").and_then(Value::as_str) == Some(run.id.as_str()),
            "refusing to delete {}: a record does not belong to this qualification run", owned.agent);
    }
    let budgets = oracle.rows(py, "cost_budgets", &owned.agent)?;
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let expected_start = datetime.call_method1("fromisoformat", (&owned.starts_at,))?;
    let prefix = format!("qualification-{}-budget-", run.id);
    for budget in &budgets {
        let name = budget["category"].as_str().ok_or_else(|| anyhow::anyhow!("budget category is not text"))?;
        let recognized = name == "all" || name.strip_prefix(&prefix).and_then(|index| index.parse::<usize>().ok())
            .is_some_and(|index| index > 0 && index < run.fixture.supabase.pagination_budgets);
        let starts = budget["starts_at"].as_str().ok_or_else(|| anyhow::anyhow!("budget starts_at is not text"))?;
        let actual_start = datetime.call_method1("fromisoformat", (starts,))?;
        ensure!(recognized && budget["period"] == "weekly" && actual_start.eq(&expected_start)?,
            "refusing to delete {}: an unowned budget was observed", owned.agent);
    }
    for table in ["cost_records", "cost_budgets"] {
        oracle.request(py, "delete owned qualification rows", "DELETE", &format!("/rest/v1/{table}"),
            &[("agent_id", format!("eq.{}", owned.agent))], None)?.require_success()?;
        ensure!(oracle.rows(py, table, &owned.agent)?.is_empty(), "{table} still contains {} after deletion", owned.agent);
    }
    Ok(json!({"agent": owned.agent, "records_removed": records.len(), "budgets_removed": budgets.len(), "confirmed_absent": true}))
}

pub fn cleanup_run(run: &Run) -> Result<Value> {
    let directory = run.directory.join("owned");
    if !directory.exists() { return Ok(json!({"agents": [], "reason": "no external ownership was claimed"})); }
    let key = credential(&run.fixture.supabase.key_env)?;
    let label = format!("cleanup-{}", super::unique_name()?);
    private_directory(&run.directory.join("http").join(&label))?;
    let mut oracle = Oracle::new(run, &label, &run.fixture.supabase.url, key, true);
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") { continue; }
        let owned: OwnedAgent = read_json(&entry.path())?;
        match Python::attach(|py| cleanup(py, run, &owned, &mut oracle)) {
            Ok(value) => outcomes.push(value),
            Err(error) => failures.push(json!({"agent": owned.agent, "error": format!("{error:#}")})),
        }
    }
    let report = json!({"agents": outcomes, "failures": failures});
    write_json(&run.directory.join("cleanup").join(format!("{label}.json")), &report)?;
    ensure!(failures.is_empty(), "cleanup refused or failed; retained report: {}", serde_json::to_string(&report)?);
    Ok(report)
}
