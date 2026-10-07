use crate::support::{python, read_json, Run};
use anyhow::{ensure, Context, Result};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::{json, Value};

pub fn run(py: Python<'_>, run: &Run) -> Result<Value> {
    let catalog: Value = read_json(&run.root.join("pricing/costs.json"))?;
    let price = |pointer: &str| -> Result<f64> {
        catalog
            .pointer(pointer)
            .and_then(Value::as_f64)
            .with_context(|| format!("canonical price missing at {pointer}"))
    };
    let tracker = python::tracker(py, &python::uuid(py)?, "memory", None, None, false)?;
    let mut expected = Vec::new();
    tracker.call_method1("record_captcha", ("capsolver", "recaptcha_v2"))?;
    expected.push((
        "captcha_capsolver",
        "solves",
        1.0,
        price("/captcha/capsolver/recaptcha_v2")?,
    ));
    tracker.call_method1("record_sms", ("juicysms", "REDDIT"))?;
    expected.push(("sms_juicysms", "units", 1.0, price("/sms/juicysms/reddit")?));
    let mobile = PyDict::new(py);
    mobile.set_item("is_mobile", true)?;
    let half_gib = 536_870_912_u64;
    tracker.call_method("record_proxy_bytes", ("oxylabs", half_gib), Some(&mobile))?;
    expected.push((
        "proxy_oxylabs_mobile",
        "bytes",
        half_gib as f64,
        price("/proxy_per_gb/oxylabs_mobile")? / 2.0,
    ));
    let tokens = PyDict::new(py);
    tokens.set_item("input_tokens", 500)?;
    tokens.set_item("output_tokens", 250)?;
    tokens.set_item("skill_id", "qualification-priced-usage")?;
    tracker.call_method("record_llm", ("gpt-4o",), Some(&tokens))?;
    expected.push((
        "llm_openai",
        "tokens",
        750.0,
        price("/llm/gpt-4o/input_per_1k")? / 2.0 + price("/llm/gpt-4o/output_per_1k")? / 4.0,
    ));
    tracker.call_method1("record_compute", ("aws_t3.medium", 1800))?;
    expected.push((
        "compute_aws",
        "seconds",
        1800.0,
        price("/compute_per_hour/aws_t3.medium")? / 2.0,
    ));
    tracker.call_method1("record_email", ("resend",))?;
    expected.push(("email_resend", "emails", 1.0, price("/email/resend")?));
    let overridden = PyDict::new(py);
    let large_tokens = (1_u64 << 60) + 1;
    overridden.set_item("input_tokens", large_tokens)?;
    overridden.set_item("override", python::COST_USD)?;
    overridden.set_item("skill_id", "qualification-explicit-charge")?;
    tracker.call_method("record_llm", ("gpt-4o-mini",), Some(&overridden))?;
    expected.push(("llm_openai", "tokens", large_tokens as f64, python::COST_USD));
    let mini_cost = price("/llm/gpt-4o-mini/input_per_1k")? / 2.0 + price("/llm/gpt-4o-mini/output_per_1k")? / 4.0;
    tracker.call_method("record_llm", ("GPT-4O-MINI",), Some(&tokens))?;
    expected.push(("llm_openai", "tokens", 750.0, mini_cost));
    tracker.call_method("record_llm", ("gpt-4o-mini-2024-07-18",), Some(&tokens))?;
    expected.push(("llm_openai", "tokens", 750.0, mini_cost));
    tracker.call_method("record_llm", ("qualification-unpriced-model",), Some(&tokens))?;
    expected.push((
        "llm_other",
        "tokens",
        750.0,
        price("/llm/default/input_per_1k")? / 2.0 + price("/llm/default/output_per_1k")? / 4.0,
    ));
    tracker.call_method1("record_captcha", ("capsolver", "qualification-unlisted-task"))?;
    expected.push(("captcha_capsolver", "solves", 1.0, price("/captcha/capsolver/default")?));

    let unpriced = format!("qualification_{}", python::uuid(py)?);
    let captcha_service = format!("captcha_{unpriced}");
    let sms_service = format!("sms_{unpriced}");
    let mut refusals = Vec::new();
    for (method, service, unit) in [
        ("record_captcha", captcha_service.as_str(), "solves"),
        ("record_sms", sms_service.as_str(), "units"),
    ] {
        let before = python::json_value(py, &tracker.call_method0("snapshot")?)?;
        let error = tracker
            .call_method1(method, (&unpriced, "qualification-resource"))
            .err()
            .context("an unpriced provider was accepted without a billed amount")?;
        ensure!(
            error.is_instance_of::<pyo3::exceptions::PyValueError>(py)
                && error.to_string().contains(&unpriced)
                && error.to_string().contains("qualification-resource"),
            "pricing refusal omitted the unknown provider or resource: {error}"
        );
        ensure!(
            python::json_value(py, &tracker.call_method0("snapshot")?)? == before,
            "unpriced usage entered the buffer"
        );
        refusals.push(json!({"operation": method, "error": error.to_string()}));
        let free = PyDict::new(py);
        free.set_item("override", 0)?;
        tracker.call_method(method, (&unpriced, "qualification-resource"), Some(&free))?;
        expected.push((service, unit, 1.0, 0.0));
    }
    tracker.call_method0("flush")?;
    let sink = tracker.call_method0("get_sink")?;
    let rows = python::record_json(py, &sink.getattr("records")?)?;
    let records = rows.as_array().context("accepted helper records are not an array")?;
    ensure!(records.len() == expected.len(), "helper usage was lost or duplicated");
    for (row, (service, unit, amount, cost)) in records.iter().zip(&expected) {
        ensure!(row["service"] == *service && row["usage_type"] == *unit
            && row["usage_amount"].as_f64() == Some(*amount) && row["cost_usd"].as_f64() == Some(*cost),
            "priced usage does not match canonical unit arithmetic: {row}; expected {service} {amount} {unit}, USD {cost}");
    }
    ensure!(
        records[3]["metadata"]["input_tokens"] == 500
            && records[3]["metadata"]["output_tokens"] == 250
            && records[3]["metadata"]["skill_id"] == "qualification-priced-usage"
            && records[6]["metadata"]["input_tokens"].as_u64() == Some(large_tokens)
            && records[6]["metadata"]["output_tokens"] == 0,
        "token accounting lost integer precision or omitted-default semantics"
    );
    Ok(json!({"accepted_records": rows, "pricing_refusals": refusals}))
}
