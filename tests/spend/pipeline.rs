use anyhow::{Context, Result, ensure};
use pyo3::prelude::*;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use crate::cases::Case;
use crate::support::{self, Fixture, Run, commands, private_directory, read_json, write_json};

fn source(run: &Run, label: &str) -> Result<Value> {
    let revision = commands::output(run, &format!("{label}-revision"), Path::new("git"),
        &commands::args(&["rev-parse", "HEAD"]), &[])?;
    let changes = commands::output(run, &format!("{label}-changes"), Path::new("git"),
        &commands::args(&["status", "--porcelain=v1", "--untracked-files=all"]), &[])?;
    ensure!(revision.trim() == run.revision && changes.trim().is_empty(),
        "qualification source changed: expected {}, observed {}; changes:\n{}", run.revision, revision.trim(), changes);
    Ok(json!({"revision": revision.trim(), "changes": changes}))
}

fn install(run: &Run) -> Result<Value> {
    let environment = run.environment("installation", false)?;
    let node = commands::output(run, "node-version", Path::new("node"), &commands::args(&["--version"]), &environment)?;
    let major: u32 = node.trim().trim_start_matches('v').split('.').next().context("Node version is missing")?.parse()?;
    ensure!(major >= 22, "the SDK requires Node 22 or later; observed {}", node.trim());
    let python = commands::output(run, "python-version", Path::new(&run.python), &commands::args(&["-VV"]), &environment)?;
    let cargo = commands::output(run, "cargo-version", Path::new("cargo"), &commands::args(&["--version"]), &environment)?;
    let rustc = commands::output(run, "rustc-version", Path::new("rustc"), &commands::args(&["--version"]), &environment)?;
    let wheels = run.directory.join("wheels");
    private_directory(&wheels)?;
    let wheel_args = vec![OsString::from("-m"), "pip".into(), "wheel".into(), "--verbose".into(),
        "--wheel-dir".into(), wheels.as_os_str().to_owned(), run.root.join("py").into_os_string()];
    commands::execute(run, "python-wheel", Path::new(&run.python), &wheel_args, &environment)?.require_success()?;
    let install_args = vec![OsString::from("-m"), "pip".into(), "install".into(), "--no-index".into(),
        "--only-binary=:all:".into(), "--find-links".into(), wheels.as_os_str().to_owned(),
        "--target".into(), run.site().into_os_string(), "wisent-cost-tracker".into()];
    commands::execute(run, "python-install", Path::new(&run.python), &install_args, &environment)?.require_success()?;
    let npm_cache = run.directory.join("cache/npm");
    commands::execute(run, "npm-ci", Path::new("npm"), &[
        "ci".into(), "--no-audit".into(), "--no-fund".into(), "--cache".into(), npm_cache.as_os_str().to_owned(),
    ], &environment)?.require_success()?;
    let packages = run.directory.join("npm-package");
    private_directory(&packages)?;
    let packed = commands::output(run, "npm-pack", Path::new("npm"), &[
        "pack".into(), "--json".into(), "--ignore-scripts".into(), "--pack-destination".into(), packages.as_os_str().to_owned(),
    ], &environment)?;
    let packed: Value = serde_json::from_str(&packed)?;
    let filename = packed[0]["filename"].as_str().context("npm pack did not identify its archive")?;
    let archive = packages.join(filename);
    commands::execute(run, "npm-install", Path::new("npm"), &[
        "install".into(), "--no-audit".into(), "--no-fund".into(), "--prefix".into(),
        run.directory.join("npm-site").into_os_string(), "--cache".into(), npm_cache.into_os_string(), archive.as_os_str().to_owned(),
    ], &environment)?.require_success()?;
    let artifacts = Python::attach(|py| -> Result<Value> {
        let mut artifacts = Vec::new();
        for entry in std::fs::read_dir(&wheels)? {
            let path = entry?.path();
            if path.is_file() { artifacts.push(json!({"file": path, "sha256": support::python::sha256(py, &path)?})); }
        }
        artifacts.push(json!({"file": archive, "sha256": support::python::sha256(py, &archive)?}));
        let executable = std::env::current_exe()?;
        artifacts.push(json!({"driver": executable, "sha256": support::python::sha256(py, &executable)?}));
        Ok(Value::Array(artifacts))
    })?;
    let identity = json!({"node": node.trim(), "python": python.trim(), "cargo": cargo.trim(),
        "rustc": rustc.trim(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "artifacts": artifacts});
    write_json(&run.directory.join("installation.json"), &identity)?;
    Ok(identity)
}

pub fn execute(fixture_path: &Path, python: &str) -> Result<()> {
    let root = support::root()?;
    let fixture: Fixture = read_json(fixture_path)?;
    ensure!(fixture.supabase.pagination_records >= 2 && fixture.supabase.pagination_budgets >= 2,
        "the declared pagination datasets must each contain at least two rows to cross a page boundary");
    let id = support::unique_name()?;
    let directory = root.join(".build/spend").join(&id);
    private_directory(&directory)?;
    let run = Run { schema_version: 1, id, revision: env!("QUALIFICATION_SOURCE_REVISION").into(),
        root, directory, python: python.into(), fixture };
    write_json(&run.directory.join("run.json"), &run)?;
    println!("Evidence: {}", run.directory.display());
    let outcome = (|| -> Result<Value> {
        let initial = source(&run, "initial")?;
        let installation = install(&run)?;
        source(&run, "installed")?;
        let mut cases = Vec::new();
        let executable = std::env::current_exe()?;
        for case in Case::ALL {
            let name = case.name()?;
            let command = commands::execute(&run, &format!("worker-{name}"), &executable, &[
                "worker".into(), "--case".into(), name.as_str().into(), "--run-dir".into(), run.directory.as_os_str().to_owned(),
            ], &[])?;
            cases.push(json!({"case": case, "passed": command.success(), "exit": command}));
        }
        Ok(json!({"source": initial, "installation": installation, "cases": cases}))
    })();
    // Cleanup also runs after an installation failure or a worker spawn failure.
    let cleanup = support::ownership::cleanup_run(&run);
    let final_source = source(&run, "final");
    let mut report = match outcome {
        Ok(report) => report,
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    let passed = report.get("error").is_none()
        && report["cases"].as_array().is_some_and(|cases| {
            cases.len() == Case::ALL.len() && cases.iter().all(|case| case["passed"] == true)
        }) && cleanup.is_ok() && final_source.is_ok();
    report["verdict"] = json!(if passed { "passed" } else { "not-qualified" });
    report["cleanup"] = match cleanup {
        Ok(value) => value, Err(error) => json!({"error": format!("{error:#}")}),
    };
    report["final_source"] = match final_source {
        Ok(value) => value, Err(error) => json!({"error": format!("{error:#}")}),
    };
    write_json(&run.directory.join("summary.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(report["verdict"] == "passed", "real qualification did not pass; see {}", run.directory.display());
    Ok(())
}
