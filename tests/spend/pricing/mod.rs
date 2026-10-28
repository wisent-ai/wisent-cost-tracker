use crate::support::{python, read_json, Run};
use anyhow::{ensure, Context, Result};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::{json, Value};

/// The quantity a captcha, SMS or email helper records for one call
/// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/docs/record-helpers.md).
const SINGLE_EVENT_USAGE: f64 = 1.0;
/// Bytes in a gibibyte, the unit proxy prices are declared per
/// (IEC 80000-13 binary prefixes, https://www.iso.org/standard/87648.html).
const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;
/// Tokens in the block an LLM price is declared per: the catalogue's
/// `input_per_1k` and `output_per_1k` keys
/// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/pricing/costs.json).
const TOKENS_PER_PRICED_BLOCK: u64 = 1000;
/// Seconds in an hour, the unit compute prices are declared per
/// (SI Brochure, https://www.bipm.org/en/publications/si-brochure).
const SECONDS_PER_HOUR: u64 = 3600;
/// The output tokens `record_llm` records when a call states none, as its
/// declared signature `input_tokens=0, output_tokens=0` says
/// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/py/native/tracker/records.rs).
const OMITTED_TOKENS: u64 = 0;

pub fn run(py: Python<'_>, run: &Run) -> Result<Value> {
    let catalog: Value = read_json(&run.root.join("pricing/costs.json"))?;
    let price = |pointer: &str| -> Result<f64> {
        catalog
            .pointer(pointer)
            .and_then(Value::as_f64)
            .with_context(|| format!("canonical price missing at {pointer}"))
    };
    let block_cost = |model: &str| -> Result<f64> {
        Ok(price(&format!("/llm/{model}/input_per_1k"))? + price(&format!("/llm/{model}/output_per_1k"))?)
    };
    let tracker = python::tracker(py, &python::uuid(py)?, "memory", None, None, false)?;
    // A known free charge, measured as what a tracker holding nothing totals.
    let free: f64 = tracker.call_method0("total")?.extract()?;
    let mut expected = Vec::new();
    tracker.call_method1("record_captcha", ("capsolver", "recaptcha_v2"))?;
    expected.push((
        "captcha_capsolver",
        "solves",
        SINGLE_EVENT_USAGE,
        price("/captcha/capsolver/recaptcha_v2")?,
    ));
    tracker.call_method1("record_sms", ("juicysms", "REDDIT"))?;
    expected.push(("sms_juicysms", "units", SINGLE_EVENT_USAGE, price("/sms/juicysms/reddit")?));
    let mobile = PyDict::new(py);
    mobile.set_item("is_mobile", true)?;
    tracker.call_method("record_proxy_bytes", ("oxylabs", BYTES_PER_GIB), Some(&mobile))?;
    expected.push((
        "proxy_oxylabs_mobile",
        "bytes",
        BYTES_PER_GIB as f64,
        price("/proxy_per_gb/oxylabs_mobile")?,
    ));
    let both_blocks = (TOKENS_PER_PRICED_BLOCK + TOKENS_PER_PRICED_BLOCK) as f64;
    let tokens = PyDict::new(py);
    tokens.set_item("input_tokens", TOKENS_PER_PRICED_BLOCK)?;
    tokens.set_item("output_tokens", TOKENS_PER_PRICED_BLOCK)?;
    tokens.set_item("skill_id", "qualification-priced-usage")?;
    tracker.call_method("record_llm", ("gpt-4o",), Some(&tokens))?;
    expected.push(("llm", "tokens", both_blocks, block_cost("gpt-4o")?));
    tracker.call_method1("record_compute", ("aws_t3.medium", SECONDS_PER_HOUR))?;
    expected.push((
        "compute",
        "seconds",
        SECONDS_PER_HOUR as f64,
        price("/compute_per_hour/aws_t3.medium")?,
    ));
    tracker.call_method1("record_email", ("resend",))?;
    expected.push(("email_resend", "emails", SINGLE_EVENT_USAGE, price("/email/resend")?));
    // A count past the integers a double holds exactly, so a lossy path shows.
    let large_tokens = u64::MAX;
    let overridden = PyDict::new(py);
    overridden.set_item("input_tokens", large_tokens)?;
    overridden.set_item("override", python::COST_USD)?;
    overridden.set_item("skill_id", "qualification-explicit-charge")?;
    tracker.call_method("record_llm", ("gpt-4o-mini",), Some(&overridden))?;
    expected.push(("llm", "tokens", large_tokens as f64, python::COST_USD));
    tracker.call_method("record_llm", ("GPT-4O-MINI",), Some(&tokens))?;
    expected.push(("llm", "tokens", both_blocks, block_cost("gpt-4o-mini")?));
    // A name that contains a listed key without being one is priced at the default.
    tracker.call_method("record_llm", ("qualification-gpt-4o-mini-variant",), Some(&tokens))?;
    expected.push(("llm", "tokens", both_blocks, block_cost("default")?));
    tracker.call_method("record_llm", ("qualification-unpriced-model",), Some(&tokens))?;
    expected.push(("llm", "tokens", both_blocks, block_cost("default")?));
    tracker.call_method1("record_captcha", ("capsolver", "qualification-unlisted-task"))?;
    expected.push(("captcha_capsolver", "solves", SINGLE_EVENT_USAGE, price("/captcha/capsolver/default")?));

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
        let charge = PyDict::new(py);
        charge.set_item("override", free)?;
        tracker.call_method(method, (&unpriced, "qualification-resource"), Some(&charge))?;
        expected.push((service, unit, SINGLE_EVENT_USAGE, free));
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
    let skill = |name: &str| {
        records
            .iter()
            .find(|row| row["metadata"]["skill_id"] == name)
            .with_context(|| format!("no record carries skill {name}"))
    };
    let priced = skill("qualification-priced-usage")?;
    let charged = skill("qualification-explicit-charge")?;
    ensure!(
        priced["metadata"]["input_tokens"].as_u64() == Some(TOKENS_PER_PRICED_BLOCK)
            && priced["metadata"]["output_tokens"].as_u64() == Some(TOKENS_PER_PRICED_BLOCK)
            && charged["metadata"]["input_tokens"].as_u64() == Some(large_tokens)
            && charged["metadata"]["output_tokens"].as_u64() == Some(OMITTED_TOKENS),
        "token accounting lost integer precision or omitted-default semantics"
    );
    Ok(json!({"accepted_records": rows, "pricing_refusals": refusals}))
}
