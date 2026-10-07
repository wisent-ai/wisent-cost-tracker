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
    let mut arguments = vec![
        run.site().join("bin/wisent-cost-tracker-onboarding").into_os_string(), verb.into(),
    ];
    if verb == "run" {
        arguments.extend([
            "--budget-usd".into(), run.fixture.onboarding.budget_usd.to_string().into(),
            "--usage-tokens".into(), run.fixture.onboarding.usage_tokens.to_string().into(),
            "--cost-usd".into(), run.fixture.onboarding.cost_usd.to_string().into(),
        ]);
    }
    let command = commands::execute(run, label, Path::new(&run.python), &arguments, &environment)?;
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
    let fixture = &run.fixture.onboarding;
    ensure!(fixture.budget_usd.is_finite() && fixture.cost_usd.is_finite()
        && !fixture.cost_usd.is_sign_negative() && fixture.budget_usd > fixture.cost_usd,
        "onboarding fixture requires a finite non-negative cost below its finite budget");
    cli_contract(run)?;
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
    ensure!(offline["usage"]["usage_amount"].as_f64() == Some(fixture.usage_tokens.get() as f64)
        && offline["usage"]["cost_usd"] == json!(fixture.cost_usd),
        "run did not record the caller's exact amounts: {offline}");
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
    let refusal = std::fs::read_to_string(&unknown.stderr)?;
    ensure!(unknown.code.is_some_and(|code| code.is_positive()) && std::fs::read(&unknown.stdout)?.is_empty()
        && std::fs::read(run.state_path(CASE))? == bytes, "unknown action did not refuse without mutating the attempt");
    let mut failing_environment = run.environment(CASE, false)?;
    let impossible_path = run.state_path(CASE).join("attempt.json");
    failing_environment.push(("WISENT_COST_TRACKER_ONBOARDING_STATE_PATH".into(), impossible_path.to_string_lossy().into_owned()));
    let failed = commands::execute(run, "onboarding-state-refusal", Path::new(&run.python), &[
        run.site().join("bin/wisent-cost-tracker-onboarding").into_os_string(), "show".into(),
    ], &failing_environment)?;
    let failure: Value = read_json(&failed.stderr)?;
    ensure!(failed.code.is_some_and(|code| code.is_positive()) && std::fs::read(failed.stdout)?.is_empty()
        && failure["error"]["code"] == "operation_failed"
        && failure["error"]["message"].as_str().is_some_and(|message| message.contains(impossible_path.to_string_lossy().as_ref()))
        && std::fs::read(run.state_path(CASE))? == bytes,
        "state write failure did not report its path on stderr or changed the original state: {failure}");
    let mut denied_run = run.clone();
    denied_run.fixture.onboarding.budget_usd = fixture.cost_usd;
    denied_run.fixture.onboarding.cost_usd = fixture.budget_usd;
    action(&denied_run, "onboarding-denied-reset", "reset", false, None)?;
    let (denied, _) = action(&denied_run, "onboarding-denied-run", "run", false, None)?;
    let denied_state = state(run)?;
    ensure!(!allowed(&denied)? && denied_state["evidence"]["decision"] == "deny"
        && denied_state["evidence"]["cost_usd"] == json!(fixture.budget_usd)
        && denied_state["evidence"]["usage_amount"].as_f64() == Some(fixture.usage_tokens.get() as f64),
        "caller amounts did not produce a persisted over-budget decision: {denied}; {denied_state}");
    Ok(json!({"offline": offline, "migrated_central_attempt": migrated, "online": online,
        "central_attempt": central, "denied": denied, "state_failure": failure,
        "auth_refusal": auth_refusal, "unknown_action": refusal, "state": run.state_path(CASE),
        "audit_retention": "Owned attempts remain as immutable central onboarding audit evidence."}))
}

fn cli_contract(run: &Run) -> Result<()> {
    let executable = run.site().join("bin/wisent-cost-tracker-onboarding").into_os_string();
    let environment = run.environment(CASE, false)?;
    for (label, arguments) in [
        ("onboarding-top-help", vec!["--help"]),
        ("onboarding-run-help", vec!["run", "--help"]),
        ("onboarding-status-text", vec!["status", "--text"]),
    ] {
        let mut argv = vec![executable.clone()];
        argv.extend(arguments.into_iter().map(Into::into));
        let command = commands::execute(run, label, Path::new(&run.python), &argv, &environment)?;
        command.require_success()?;
        ensure!(!run.state_path(CASE).exists(), "{label} created onboarding state");
        let output = std::fs::read_to_string(command.stdout)?;
        if label == "onboarding-status-text" {
            ensure!(output.contains("result.status: not_started") && serde_json::from_str::<Value>(&output).is_err(),
                "text status did not describe the same not-started state: {output}");
        }
    }
    let budget = run.fixture.onboarding.budget_usd.to_string();
    let tokens = run.fixture.onboarding.usage_tokens.to_string();
    let cost = run.fixture.onboarding.cost_usd.to_string();
    for (label, arguments) in [
        ("onboarding-missing-amounts", vec!["run"]),
        ("onboarding-unexpected-amount", vec!["status", "--cost-usd", "NaN"]),
        ("onboarding-nonfinite-cost", vec!["run", "--budget-usd", &budget, "--usage-tokens", &tokens, "--cost-usd", "NaN"]),
        ("onboarding-zero-tokens", vec!["run", "--budget-usd", &budget, "--usage-tokens", "0", "--cost-usd", &cost]),
        ("onboarding-conflicting-format", vec!["status", "--text", "--json"]),
    ] {
        let mut argv = vec![executable.clone()];
        argv.extend(arguments.into_iter().map(Into::into));
        let command = commands::execute(run, label, Path::new(&run.python), &argv, &environment)?;
        ensure!(command.code.is_some_and(|code| code.is_positive())
            && std::fs::read(command.stdout)?.is_empty()
            && !run.state_path(CASE).exists(), "{label} did not refuse before changing state");
    }
    Ok(())
}
