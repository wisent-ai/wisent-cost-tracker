mod cases;
mod pipeline;
mod pricing;
mod provider;
mod support;

use anyhow::{Result, ensure};
use pyo3::prelude::*;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use cases::Case;
use support::Run;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Action { Run, Status, Cleanup, Worker, WorkerLifecycle }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunOptions { fixture: PathBuf, python: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryOptions { #[serde(rename = "run-dir")] run_dir: PathBuf }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerOptions { #[serde(rename = "run-dir")] run_dir: PathBuf, case: Case }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleOptions {
    #[serde(rename = "run-dir")]
    run_dir: PathBuf,
    mode: cases::lifecycle::Mode,
}

enum Request {
    Run(RunOptions), Status(DirectoryOptions), Cleanup(DirectoryOptions),
    Worker(WorkerOptions), Lifecycle(LifecycleOptions),
}

fn options<T: for<'de> Deserialize<'de>>(arguments: &[String]) -> Result<T> {
    let mut values = Map::new();
    let mut pairs = arguments.chunks_exact(2);
    for pair in &mut pairs {
        let key = pair[0].strip_prefix("--").ok_or_else(|| anyhow::anyhow!("option requires --: {}", pair[0]))?;
        ensure!(values.insert(key.into(), Value::String(pair[1].clone())).is_none(), "duplicate option --{key}");
    }
    ensure!(pairs.remainder().is_empty(), "every option requires a value");
    Ok(serde_json::from_value(Value::Object(values))?)
}

fn parse(arguments: &[String]) -> Result<Request> {
    let action: Action = serde_json::from_value(Value::String(arguments[0].clone()))?;
    Ok(match action {
        Action::Run => Request::Run(options(&arguments[1..])?),
        Action::Status => Request::Status(options(&arguments[1..])?),
        Action::Cleanup => Request::Cleanup(options(&arguments[1..])?),
        Action::Worker => Request::Worker(options(&arguments[1..])?),
        Action::WorkerLifecycle => Request::Lifecycle(options(&arguments[1..])?),
    })
}

fn worker(options: WorkerOptions) -> Result<i32> {
    let run = Run::load(&options.run_dir)?;
    let name = options.case.name()?;
    if let Err(error) = options.case.prerequisites(&run) {
        run.result(&name, &json!({"case": options.case, "verdict": "blocked", "error": format!("{error:#}")}))?;
        eprintln!("{error:#}");
        return Ok(78);
    }
    let outcome = (|| -> Result<Value> {
        support::python::configure(&run, &name, matches!(options.case, Case::Onboarding))?;
        let identity = Python::attach(|py| support::python::installed_identity(py, &run))?;
        let observations = options.case.execute(&run)?;
        Ok(json!({"case": options.case, "verdict": "passed", "identity": identity, "observations": observations}))
    })();
    let (report, code) = match outcome {
        Ok(report) => (report, 0),
        Err(error) => {
            Python::attach(|py| {
                for cause in error.chain() {
                    if let Some(error) = cause.downcast_ref::<PyErr>() { error.print(py); }
                }
            });
            eprintln!("{error:#}");
            (json!({"case": options.case, "verdict": "failed", "error": format!("{error:#}")}), 1)
        }
    };
    run.result(&name, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(code)
}

fn execute(request: Request) -> Result<i32> {
    match request {
        Request::Run(options) => { pipeline::execute(&options.fixture, &options.python)?; Ok(0) }
        Request::Worker(options) => worker(options),
        Request::Status(options) => {
            let run = Run::load(&options.run_dir)?;
            let summary: Value = support::read_json(&run.directory.join("summary.json"))?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(if summary["verdict"] == "passed" { 0 } else { 1 })
        }
        Request::Cleanup(options) => {
            let report = support::ownership::cleanup_run(&Run::load(&options.run_dir)?)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(0)
        }
        Request::Lifecycle(options) => {
            let run = Run::load(&options.run_dir)?;
            cases::lifecycle::worker(&run, options.mode)
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() || arguments[0] == "--help" {
        println!("spend-journey run --fixture PATH --python EXECUTABLE\nspend-journey status --run-dir PATH\nspend-journey cleanup --run-dir PATH");
        return;
    }
    let request = match parse(&arguments) {
        Ok(request) => request,
        Err(error) => { eprintln!("invalid qualification command: {error:#}"); std::process::exit(2); }
    };
    match execute(request) {
        Ok(code) => std::process::exit(code),
        Err(error) => { eprintln!("{error:#}"); std::process::exit(1); }
    }
}
