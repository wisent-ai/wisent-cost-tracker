use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::{Value, json};
use std::path::Path;
use crate::{provider, support::{Run, commands, credential, oracle::Oracle, ownership, python, read_json}};
use python::COST_USD;

fn native(py: Python<'_>, run: &Run, owned: &ownership::OwnedAgent, key: &str) -> Result<Value> {
    let tracker = python::tracker(py, &owned.agent, "supabase", None, Some((&run.fixture.supabase.url, key)), false)?;
    let sink = tracker.call_method0("get_sink")?;
    let manager = python::manager(py, &owned.agent, &sink)?;
    let starts = py.import("datetime")?.getattr("datetime")?.call_method1("fromisoformat", (&owned.starts_at,))?;
    manager.call_method1("set_budget", ("all", 2.0 * COST_USD, "weekly", &starts))?;
    python::record(py, &tracker, &run.id, "record-0", COST_USD)?;
    tracker.call_method0("flush")?;
    let allowed = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(allowed["decision"] == "allow" && allowed["remaining_usd"].as_f64() == Some(COST_USD), "first persisted cost did not permit the next operation: {allowed}");
    let second = python::record(py, &tracker, &run.id, "record-1", 2.0 * COST_USD)?;
    tracker.call_method0("flush")?;
    tracker.call_method0("flush")?;
    let denied = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(denied["decision"] == "deny" && denied["remaining_usd"].as_f64() == Some(-COST_USD), "accepted overspend did not refuse the next operation: {denied}");
    for index in 2..run.fixture.supabase.pagination_records {
        python::record(py, &tracker, &run.id, &format!("record-{index}"), COST_USD)?;
    }
    tracker.call_method0("flush")?;
    tracker.call_method0("flush")?;
    let rows = python::record_json(py, &sink.call_method1("read", (&owned.agent, python::since(py)?))?)?;
    provider::records(run, &rows)?;
    let budgets = python::json_value(py, &sink.call_method1("read_budgets", (&owned.agent,))?)?;
    provider::budgets(run, &budgets)?;
    manager.call_method1("set_budget", ("all", (run.fixture.supabase.pagination_records + 2) as f64 * COST_USD, "weekly", &starts))?;
    let restored = python::json_value(py, &manager.call_method1("decide", ("all",))?)?;
    ensure!(restored["decision"] == "allow" && restored["remaining_usd"].as_f64() == Some(COST_USD), "budget edit did not restore permission: {restored}");
    let constructor = py.import("wisent_cost_tracker")?.getattr("SupabaseSink")?;
    let invalid = constructor.call1((&run.fixture.supabase.url, python::uuid(py)?))?;
    let refused = invalid.call_method1("read", (&owned.agent, python::since(py)?)).err()
        .context("a random key was accepted by the real provider")?;
    let status: u16 = refused.value(py).getattr("status_code")?.extract()?;
    ensure!(status == 401 || status == 403, "invalid credentials were not an authentication refusal: {refused}");
    let missing = format!("qualification_missing_{}_{}", python::uuid(py)?.replace('-', ""), "absent_".repeat(40));
    let options = PyDict::new(py);
    options.set_item("table", &missing)?;
    let invalid = constructor.call((&run.fixture.supabase.url, key), Some(&options))?;
    let records = PyList::new(py, [&second])?;
    let error = invalid.call_method1("write", (records,)).err()
        .context("writing a missing provider table unexpectedly succeeded")?;
    let failure = error.value(py);
    let body: String = failure.getattr("response_body")?.extract()?;
    let body_json: Value = serde_json::from_str(&body).context("provider error body was not preserved as complete JSON")?;
    ensure!(body.chars().count() > 200 && error.to_string().contains(&body)
        && failure.getattr("method")?.extract::<String>()? == "POST"
        && failure.getattr("url")?.extract::<String>()?.ends_with(&missing),
        "provider error did not cross and preserve the former response clipping boundary: {error}");
    Ok(json!({"allowed": allowed, "denied": denied, "restored": restored,
        "records": rows, "budgets": budgets, "authentication_status": status,
        "provider_error": {"status": failure.getattr("status_code")?.extract::<u16>()?, "body": body_json, "message": error.to_string()}}))
}

pub fn run(run: &Run) -> Result<Value> {
    let key = credential(&run.fixture.supabase.key_env)?;
    Python::attach(|py| {
        let mut oracle = Oracle::new(run, "supabase", &run.fixture.supabase.url, key.clone(), true);
        let native_owned = ownership::reserve(py, run, "native", &mut oracle)?;
        let node_owned = ownership::reserve(py, run, "node", &mut oracle)?;
        let outcome = (|| -> Result<Value> {
            provider::seed_budgets(py, run, &native_owned, &mut oracle)?;
            provider::seed_budgets(py, run, &node_owned, &mut oracle)?;
            let native = native(py, run, &native_owned, &key)?;
            let native_state = provider::observe(py, run, &native_owned, &mut oracle)?;
            let command = commands::execute(run, "node-supabase", Path::new("node"), &[
                run.root.join("tests/spend/js/supabase.mjs").into_os_string(), run.directory.as_os_str().to_owned(),
            ], &run.environment("node-supabase", false)?)?;
            command.require_success()?;
            let node: Value = read_json(&command.stdout)?;
            let node_state = provider::observe(py, run, &node_owned, &mut oracle)?;
            Ok(json!({"native": native, "native_persisted": native_state, "node": node, "node_persisted": node_state}))
        })();
        let native_cleanup = ownership::cleanup(py, run, &native_owned, &mut oracle);
        let node_cleanup = ownership::cleanup(py, run, &node_owned, &mut oracle);
        match (outcome, native_cleanup, node_cleanup) {
            (Ok(result), Ok(native), Ok(node)) => Ok(json!({"observations": result, "cleanup": [native, node]})),
            (result, native, node) => anyhow::bail!("Supabase journey: {:?}; native cleanup: {:?}; Node cleanup: {:?}",
                result.err(), native, node),
        }
    })
}
