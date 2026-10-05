use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use crate::support::{Run, ownership::{self, OwnedAgent}, oracle::Oracle, python::COST_USD};

pub fn seed_budgets(py: Python<'_>, run: &Run, owned: &OwnedAgent, oracle: &mut Oracle<'_>) -> Result<()> {
    let rows: Vec<Value> = (1..run.fixture.supabase.pagination_budgets).map(|index| json!({
        "agent_id": owned.agent, "category": ownership::category(run, index),
        "allocated_usd": COST_USD, "period": "weekly", "starts_at": owned.starts_at,
    })).collect();
    oracle.request(py, "seed owned pagination budgets", "POST", "/rest/v1/cost_budgets", &[],
        Some(&Value::Array(rows)))?.require_success()
}

pub fn records(run: &Run, rows: &Value) -> Result<()> {
    let rows = rows.as_array().context("record result is not an array")?;
    ensure!(rows.len() == run.fixture.supabase.pagination_records, "record count differs from the declared dataset");
    let mut markers = BTreeSet::new();
    for row in rows {
        let marker = row["reference_id"].as_str().context("record has no marker")?;
        let index: usize = marker.strip_prefix("record-").context("record has an unknown marker")?.parse()?;
        ensure!(index < rows.len() && markers.insert(index), "duplicate or foreign record marker {marker}");
        ensure!(row["cost_usd"].as_f64() == Some(if index == 1 { 2.0 * COST_USD } else { COST_USD })
            && row["metadata"]["qualification_run"] == run.id, "stored cost or ownership changed: {row}");
    }
    Ok(())
}

pub fn budgets(run: &Run, rows: &Value) -> Result<()> {
    let rows = rows.as_array().context("budget result is not an array")?;
    let expected: BTreeSet<String> = std::iter::once("all".into())
        .chain((1..run.fixture.supabase.pagination_budgets).map(|index| ownership::category(run, index))).collect();
    let observed: BTreeSet<String> = rows.iter().map(|row| row["category"].as_str()
        .map(str::to_owned).context("budget category is not text")).collect::<Result<_>>()?;
    ensure!(rows.len() == expected.len() && observed == expected, "budget pagination lost or duplicated categories");
    Ok(())
}

pub fn observe(py: Python<'_>, run: &Run, owned: &OwnedAgent, oracle: &mut Oracle<'_>) -> Result<Value> {
    let stored_records = Value::Array(oracle.rows(py, "cost_records", &owned.agent)?);
    records(run, &stored_records)?;
    let stored_budgets = Value::Array(oracle.rows(py, "cost_budgets", &owned.agent)?);
    budgets(run, &stored_budgets)?;
    let mut pages = Vec::new();
    for (table, expected) in [("cost_records", run.fixture.supabase.pagination_records),
        ("cost_budget_status", run.fixture.supabase.pagination_budgets)] {
        let reply = oracle.request(py, "observe real provider page boundary", "GET", &format!("/rest/v1/{table}"),
            &[("agent_id", format!("eq.{}", owned.agent)), ("order", "id.asc".into())], None)?;
        reply.require_success()?;
        let page = reply.value()?;
        let returned = page.as_array().context("provider page is not an array")?.len();
        let total = reply.range.as_deref().and_then(|range| range.rsplit_once('/'))
            .and_then(|(_, count)| count.parse::<usize>().ok());
        ensure!(returned > 0 && returned < expected && total == Some(expected),
            "pagination qualification requires a real page boundary: {table} returned {returned}, dataset {expected}, Content-Range {:?}; increase the declared dataset", reply.range);
        pages.push(json!({"table": table, "returned": returned, "total": total, "content_range": reply.range}));
    }
    let status = oracle.request(py, "inspect persisted edited budget", "GET", "/rest/v1/cost_budget_status", &[
        ("agent_id", format!("eq.{}", owned.agent)), ("category", "eq.all".into()),
    ], None)?;
    status.require_success()?;
    let status = status.value()?;
    ensure!(status.as_array().is_some_and(|rows| rows.len() == 1)
        && status[0]["allocated_usd"].as_f64() == Some((run.fixture.supabase.pagination_records + 2) as f64 * COST_USD)
        && status[0]["spent_usd"].as_f64() == Some((run.fixture.supabase.pagination_records + 1) as f64 * COST_USD)
        && status[0]["remaining_usd"].as_f64() == Some(COST_USD)
        && status[0]["is_over_budget"] == false, "persisted edited budget does not match accepted usage: {status}");
    Ok(json!({"records": stored_records, "budgets": stored_budgets, "status": status, "pages": pages}))
}
