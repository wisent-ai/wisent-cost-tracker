//! Native command actions over the existing shared journey runtime.
mod cli;
mod workflow;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyInt, PyTuple};

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Show,
    Status,
    Skip,
    Abandon,
    Reset,
    Run,
}

impl Action {
    const ALL: &[Self] = &[
        Self::Show,
        Self::Status,
        Self::Skip,
        Self::Abandon,
        Self::Reset,
        Self::Run,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::Status => "status",
            Self::Skip => "skip",
            Self::Abandon => "abandon",
            Self::Reset => "reset",
            Self::Run => "run",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Show => "Start or resume the first-use journey",
            Self::Status => "Read the journey without starting an attempt",
            Self::Skip => "Skip the current journey",
            Self::Abandon => "Abandon the current journey",
            Self::Reset => "Start a new attempt",
            Self::Run => "Record caller-supplied usage and take a real budget decision",
        }
    }
}

fn definition(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    py.import("wisent_cost_tracker.onboarding.definition")
}

fn invalid(py: Python<'_>, code: &str, message: impl AsRef<str>) -> PyErr {
    match definition(py).and_then(|module| module.getattr("OnboardingError")?.call1((code, message.as_ref()))) {
        Ok(error) => PyErr::from_value(error),
        Err(error) => error,
    }
}

fn validate<'py>(
    py: Python<'py>,
    action: Action,
    budget: Option<&Bound<'py, PyAny>>,
    tokens: Option<&Bound<'py, PyAny>>,
    cost: Option<&Bound<'py, PyAny>>,
) -> PyResult<()> {
    for (name, value) in [("budget_usd", budget), ("usage_tokens", tokens), ("cost_usd", cost)] {
        match (action, value) {
            (Action::Run, Some(value)) => {
                crate::bridge::numeric(py, value, name, false)
                    .map_err(|cause| invalid(py, "invalid_amount", cause.to_string()))?;
                if name == "usage_tokens" && !value.get_type().is(&py.get_type::<PyInt>()) {
                    return Err(invalid(
                        py,
                        "invalid_amount",
                        "usage_tokens must be a non-negative whole number",
                    ));
                }
            }
            (Action::Run, None) => {
                return Err(invalid(
                    py,
                    "missing_amount",
                    format!(
                        "run requires {name}; supply --{} on the command line",
                        name.replace('_', "-")
                    ),
                ))
            }
            (_, Some(_)) => {
                return Err(invalid(
                    py,
                    "unexpected_amount",
                    format!("{name} is only valid for run; no onboarding state was changed"),
                ))
            }
            (_, None) => {}
        }
    }
    Ok(())
}

#[pyfunction]
#[pyo3(signature = (action="show", *, budget_usd=None, usage_tokens=None, cost_usd=None))]
fn run_onboarding_action<'py>(
    py: Python<'py>,
    action: &str,
    budget_usd: Option<&Bound<'py, PyAny>>,
    usage_tokens: Option<&Bound<'py, PyAny>>,
    cost_usd: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let action = Action::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.name() == action)
        .ok_or_else(|| {
            invalid(
                py,
                "unknown_action",
                format!("unknown onboarding action {action:?}; use --help to list actions"),
            )
        })?;
    validate(py, action, budget_usd, usage_tokens, cost_usd)?;
    if action == Action::Run {
        return workflow::run(
            py,
            budget_usd.expect("validated budget"),
            usage_tokens.expect("validated usage"),
            cost_usd.expect("validated cost"),
        );
    }
    let runtime = py
        .import("wisent_cost_tracker.onboarding.engine")?
        .getattr("Runtime")?
        .call0()?;
    if !runtime.call_method1("open", (action != Action::Status,))?.is_truthy()? {
        let result = PyDict::new(py);
        let definition = definition(py)?;
        for (field, constant) in [
            ("product_id", "PRODUCT_ID"),
            ("journey_id", "JOURNEY_ID"),
            ("journey_version", "JOURNEY_VERSION"),
            ("journey_version_id", "JOURNEY_VERSION_ID"),
            ("source_revision", "SOURCE_REVISION"),
            ("first_success_fact", "FIRST_SUCCESS_FACT"),
        ] {
            result.set_item(field, definition.getattr(constant)?)?;
        }
        result.set_item("status", "not_started")?;
        return Ok(result.into_any());
    }
    match action {
        Action::Reset => {
            runtime.call_method0("reset")?;
        }
        Action::Skip => {
            runtime.call_method1("set_terminal", ("skipped", "onboarding_step_skipped"))?;
        }
        Action::Abandon => {
            runtime.call_method1("set_terminal", ("abandoned", "onboarding_abandoned"))?;
        }
        Action::Show if runtime.getattr("was_existing")?.is_truthy()? => {
            runtime.call_method0("resume")?;
            runtime.call_method0("expose")?;
        }
        _ => {}
    }
    runtime.call_method0("view")
}

pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add(
        "ACTIONS",
        PyTuple::new(module.py(), Action::ALL.iter().map(|action| action.name()))?,
    )?;
    module.add_function(wrap_pyfunction!(run_onboarding_action, module)?)?;
    module.add_function(wrap_pyfunction!(cli::main, module)?)?;
    Ok(())
}
