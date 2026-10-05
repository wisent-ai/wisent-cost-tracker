use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use crate::support::{Run, commands, credential, oracle::Oracle, python, read_json, write_json};

const CASE: &str = "onboarding-cli";

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status { NotStarted, InProgress, Completed, Skipped, Abandoned }

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Decision { Allow, Deny }

fn status(value: &Value) -> Result<Status> { Ok(serde_json::from_value(value["status"].clone())?) }
fn allowed(value: &Value) -> Result<bool> {
    Ok(serde_json::from_value::<Decision>(value["budget_decision"]["decision"].clone())? == Decision::Allow)
}

fn action(run: &Run, label: &str, verb: &str, central: bool, invalid_token: Option<&str>) -> Result<(Value, String)> {
    let mut environment = run.environment(CASE, central)?;
    if let Some(token) = invalid_token {
        environment.push(("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN".into(), token.into()));
    }
    let command = commands::execute(run, label, Path::new(&run.python), &[
        run.site().join("bin/wisent-cost-tracker-onboarding").into_os_string(), verb.into(),
    ], &environment)?;
    command.require_success()?;
    let envelope: Value = read_json(&command.stdout)?;
    ensure!(envelope["ok"] == true, "onboarding command refused: {envelope}");
    Ok((envelope["result"].clone(), std::fs::read_to_string(command.stderr)?))
}

fn state(run: &Run) -> Result<Value> { read_json(&run.state_path(CASE)) }

fn central_state(py: Python<'_>, oracle: &mut Oracle<'_>, local: &Value) -> Result<Value> {
    let progress = &local["progress"];
    let reply = oracle.request(py, "observe central onboarding attempt", "POST",
        "/api/integration/onboarding/wisent-cost-tracker.state.read", &[], Some(&json!({
            "product_id": progress["product_id"], "attempt_id": progress["attempt_id"],
            "subject_hash": progress["subject_hash"],
        })))?;
    reply.require_success()?;
    let envelope = reply.value()?;
    ensure!(envelope["ok"] == true, "central state request was refused: {envelope}");
    let attempt = &envelope["result"]["attempt"];
    ensure!(attempt["id"] == progress["attempt_id"] && attempt["product_id"] == progress["product_id"]
        && attempt["subject_hash"] == progress["subject_hash"]
        && attempt["journey_version_id"] == progress["journey_version_id"]
        && attempt["status"] == progress["status"]
        && attempt["completed_screen_ids"] == progress["completed_screen_ids"],
        "central persisted attempt differs from the CLI's local state: {envelope}");
    Ok(envelope)
}

pub fn run(run: &Run) -> Result<Value> {
    let (initial, _) = action(run, "onboarding-initial-status", "status", false, None)?;
    ensure!(status(&initial)? == Status::NotStarted && !run.state_path(CASE).exists(),
        "offline status started or persisted an attempt");
    let (shown, _) = action(run, "onboarding-show", "show", false, None)?;
    ensure!(status(&shown)? == Status::InProgress && state(run)?["progress"]["attempt_id"] == shown["attempt_id"],
        "show did not persist its new attempt");
    let (skipped, _) = action(run, "onboarding-skip", "skip", false, None)?;
    ensure!(status(&skipped)? == Status::Skipped && status(&state(run)?["progress"])? == Status::Skipped,
        "skip was not persisted");
    let (reset, _) = action(run, "onboarding-reset-skipped", "reset", false, None)?;
    ensure!(reset["attempt_id"] != skipped["attempt_id"] && status(&reset)? == Status::InProgress,
        "reset did not create a fresh attempt");
    let (abandoned, _) = action(run, "onboarding-abandon", "abandon", false, None)?;
    ensure!(status(&abandoned)? == Status::Abandoned && status(&state(run)?["progress"])? == Status::Abandoned,
        "abandon was not persisted");
    action(run, "onboarding-reset-abandoned", "reset", false, None)?;
    let (offline, _) = action(run, "onboarding-offline-run", "run", false, None)?;
    let mut queued = state(run)?;
    ensure!(offline["usage_accepted"] == true && allowed(&offline)?
        && status(&queued["progress"])? == Status::Completed
        && queued["evidence"]["budget_decision_observed"] == true,
        "actual tracker workflow did not complete first use: {offline}; {queued}");
    let events = queued["pending_events"].as_array_mut().context("offline events were not persisted")?;
    ensure!(!events.is_empty(), "offline first use falsely claimed central acknowledgement");
    // Previous release format, using events from the real workflow rather than
    // invented events. Their identities, order and evidence remain unchanged.
    for event in events {
        event["journey_id"] = json!("first-use");
        event["journey_version"] = offline["onboarding"]["journey_version"].clone();
        event["experiment_id"] = Value::Null;
        event["variant_id"] = json!("control");
    }
    write_json(&run.directory.join("legacy-queued-state.json"), &queued)?;
    let mut file = std::fs::OpenOptions::new().write(true).truncate(true).open(run.state_path(CASE))?;
    serde_json::to_writer(&mut file, &queued)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    let token = Python::attach(python::uuid)?;
    let (_, auth_refusal) = action(run, "onboarding-auth-refusal", "status", true, Some(&token))?;
    let retained = state(run)?;
    let before = queued["pending_events"].as_array().context("missing original queue")?;
    let after = retained["pending_events"].as_array().context("missing retained queue")?;
    ensure!(before.iter().map(|event| &event["event_id"]).eq(after.iter().map(|event| &event["event_id"]))
        && auth_refusal.contains("wisent-integrations") && (auth_refusal.contains("401") || auth_refusal.contains("403")),
        "real authentication failure lost queued events or its actual HTTP cause: {auth_refusal}");
    ensure!(after[0].get("journey_id").is_none() && after[0].get("journey_version").is_none()
        && after[0].get("experiment_id").is_none() && after[0].get("variant_id").is_none(),
        "legacy event migration was not persisted before its refused send");
    action(run, "onboarding-reconnect", "status", true, None)?;
    let delivered = state(run)?;
    ensure!(delivered["pending_events"].as_array().is_some_and(Vec::is_empty),
        "central service did not acknowledge the actual queued events: {delivered}");
    let token = credential(&run.fixture.onboarding.token_env)?;
    let mut oracle = Oracle::new(run, "onboarding", &run.fixture.onboarding.url, token, false);
    let migrated = Python::attach(|py| central_state(py, &mut oracle, &delivered))?;
    action(run, "onboarding-central-reset", "reset", true, None)?;
    let (online, _) = action(run, "onboarding-central-run", "run", true, None)?;
    let completed = state(run)?;
    ensure!(status(&completed["progress"])? == Status::Completed
        && completed["progress"]["attempt_id"] != delivered["progress"]["attempt_id"]
        && completed["pending_events"].as_array().is_some_and(Vec::is_empty) && allowed(&online)?,
        "online first use was not completed and acknowledged");
    let central = Python::attach(|py| central_state(py, &mut oracle, &completed))?;
    let bytes = std::fs::read(run.state_path(CASE))?;
    let unknown = commands::execute(run, "onboarding-unknown-action", Path::new(&run.python), &[
        run.site().join("bin/wisent-cost-tracker-onboarding").into_os_string(), "unknown-qualification-action".into(),
    ], &run.environment(CASE, true)?)?;
    let refusal: Value = read_json(&unknown.stdout)?;
    ensure!(unknown.code == Some(2) && refusal["ok"] == false && refusal["error"]["code"] == "unknown_action"
        && std::fs::read(run.state_path(CASE))? == bytes, "unknown action did not refuse without mutating the attempt");
    Ok(json!({"offline": offline, "migrated_central_attempt": migrated, "online": online,
        "central_attempt": central, "auth_refusal": auth_refusal, "unknown_action": refusal, "state": run.state_path(CASE),
        "audit_retention": "Owned attempts remain as immutable central onboarding audit evidence."}))
}
