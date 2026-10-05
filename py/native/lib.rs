mod bridge;
mod budget;
mod sinks;
mod tracker;
mod transport;

use pyo3::prelude::*;
use pyo3::types::PyList;

#[pymodule]
fn spend(module: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = module.py();
    module.add("__path__", PyList::empty(py))?;
    module.setattr("__name__", "wisent_cost_tracker.spend")?;
    let modules = py.import("sys")?.getattr("modules")?;
    let sinks = PyModule::new(py, "wisent_cost_tracker.spend.sinks")?;
    sinks::register(&sinks)?;
    let budget = PyModule::new(py, "wisent_cost_tracker.spend.budget")?;
    budget.add_class::<budget::BudgetManager>()?;
    let tracker = PyModule::new(py, "wisent_cost_tracker.spend.tracker")?;
    tracker::register(&tracker)?;
    for (name, child) in [("sinks", &sinks), ("budget", &budget), ("tracker", &tracker)] {
        module.add(name, child)?;
        modules.set_item(format!("wisent_cost_tracker.spend.{name}"), child)?;
    }
    for (name, owner) in [
        ("BudgetManager", &budget), ("CostTracker", &tracker), ("CostTrackerOptions", &tracker),
        ("FileSink", &sinks), ("MemorySink", &sinks), ("SupabaseSink", &sinks),
    ] {
        module.add(name, owner.getattr(name)?)?;
    }
    module.add("__all__", ["BudgetManager", "CostTracker", "CostTrackerOptions", "FileSink", "MemorySink", "SupabaseSink"])?;
    Ok(())
}
