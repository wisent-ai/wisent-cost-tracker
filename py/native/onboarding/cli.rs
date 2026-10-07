use pyo3::exceptions::{PyException, PySystemExit};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use super::Action;

fn parser(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("prog", "wisent-cost-tracker-onboarding")?;
    kwargs.set_item(
        "description",
        "Manage the first-use journey. run records the supplied amounts locally; it makes no paid provider call.",
    )?;
    kwargs.set_item(
        "epilog",
        Action::ALL
            .iter()
            .map(|action| format!("{}: {}", action.name(), action.description()))
            .collect::<Vec<_>>()
            .join("; "),
    )?;
    let parser = py
        .import("argparse")?
        .getattr("ArgumentParser")?
        .call((), Some(&kwargs))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item(
        "choices",
        Action::ALL.iter().map(|action| action.name()).collect::<Vec<_>>(),
    )?;
    kwargs.set_item("nargs", "?")?;
    kwargs.set_item("default", Action::Show.name())?;
    parser.call_method("add_argument", ("action",), Some(&kwargs))?;
    let formats = parser.call_method0("add_mutually_exclusive_group")?;
    for (flag, action, help) in [
        ("--text", "store_true", "Print readable field/value lines"),
        ("--json", "store_false", "Print a JSON envelope (the default)"),
    ] {
        let kwargs = PyDict::new(py);
        kwargs.set_item("dest", "text")?;
        kwargs.set_item("action", action)?;
        kwargs.set_item("help", help)?;
        formats.call_method("add_argument", (flag,), Some(&kwargs))?;
    }
    let defaults = PyDict::new(py);
    defaults.set_item("text", false)?;
    parser.call_method("set_defaults", (), Some(&defaults))?;
    let builtins = py.import("builtins")?;
    for (flag, kind, help) in [
        (
            "--budget-usd",
            "float",
            "Required for run: daily budget in USD, finite and non-negative",
        ),
        (
            "--usage-tokens",
            "int",
            "Required for run: positive whole token count",
        ),
        (
            "--cost-usd",
            "float",
            "Required for run: recorded cost in USD, finite and non-negative",
        ),
    ] {
        let kwargs = PyDict::new(py);
        kwargs.set_item("type", builtins.getattr(kind)?)?;
        kwargs.set_item("help", help)?;
        parser.call_method("add_argument", (flag,), Some(&kwargs))?;
    }
    Ok(parser)
}

fn json(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    let kwargs = PyDict::new(py);
    kwargs.set_item("sort_keys", true)?;
    kwargs.set_item("separators", (",", ":"))?;
    py.import("json")?
        .call_method("dumps", (value,), Some(&kwargs))?
        .extract()
}

fn text(value: &Bound<'_, PyAny>, path: &str, lines: &mut Vec<String>) -> PyResult<()> {
    if let Ok(fields) = value.cast::<PyDict>() {
        if !fields.is_empty() {
            for (key, value) in fields.iter() {
                let child = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                text(&value, &child, lines)?;
            }
            return Ok(());
        }
    }
    if let Ok(values) = value.cast::<PyList>() {
        if !values.is_empty() {
            for (index, value) in values.iter().enumerate() {
                text(&value, &format!("{path}[{index}]"), lines)?;
            }
            return Ok(());
        }
    }
    lines.push(format!("{path}: {value}"));
    Ok(())
}

/// argparse owns help and usage failures (including their exit status).
/// A normal return becomes successful sys.exit(None) in the console entry.
/// sys.exit(string) prints operation failures to stderr and signals failure.
#[pyfunction]
#[pyo3(signature = (argv=None))]
pub(super) fn main(py: Python<'_>, argv: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
    let parser = parser(py)?;
    let arguments = parser.call_method1("parse_args", (argv,))?;
    let action: String = arguments.getattr("action")?.extract()?;
    let action_kind = Action::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.name() == action)
        .expect("argparse validates the declared action choices");
    let budget = arguments.getattr("budget_usd")?;
    let tokens = arguments.getattr("usage_tokens")?;
    let cost = arguments.getattr("cost_usd")?;
    let supplied = |value: &Bound<'_, PyAny>| !value.is_none();
    let budget = supplied(&budget).then_some(&budget);
    let tokens = supplied(&tokens).then_some(&tokens);
    let cost = supplied(&cost).then_some(&cost);
    if let Err(error) = super::validate(py, action_kind, budget, tokens, cost) {
        parser.call_method1("error", (error.to_string(),))?;
        return Err(error);
    }
    let output = super::run_onboarding_action(py, &action, budget, tokens, cost);
    let envelope = PyDict::new(py);
    let readable = arguments.getattr("text")?.is_truthy()?;
    match output {
        Ok(result) => {
            envelope.set_item("ok", true)?;
            envelope.set_item("result", result)?;
        }
        Err(error) if error.is_instance_of::<PyException>(py) => {
            let location = match super::definition(py)?.call_method0("state_path") {
                Ok(path) => path.to_string(),
                Err(cause) => format!("unresolved state path ({cause})"),
            };
            let message = format!("onboarding {action} failed; state file {location}: {error}");
            let detail = PyDict::new(py);
            detail.set_item("code", "operation_failed")?;
            detail.set_item("message", &message)?;
            envelope.set_item("ok", false)?;
            envelope.set_item("error", detail)?;
            return Err(PySystemExit::new_err(if readable {
                message
            } else {
                json(py, envelope.as_any())?
            }));
        }
        Err(error) => return Err(error),
    }
    let rendered = if readable {
        let mut lines = Vec::new();
        text(envelope.as_any(), "", &mut lines)?;
        lines.join("\n")
    } else {
        json(py, envelope.as_any())?
    };
    py.import("sys")?
        .getattr("stdout")?
        .call_method1("write", (format!("{rendered}\n"),))?;
    Ok(())
}
