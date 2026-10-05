use std::sync::atomic::Ordering;
use pyo3::exceptions::{PyException, PyOSError, PyValueError};
use pyo3::prelude::*;
use super::CostTracker;

pub fn register(py: Python<'_>, tracker: &Py<CostTracker>) -> PyResult<()> {
    let tracker_ref = tracker.bind(py);
    py.import("atexit")?.call_method1("register", (tracker_ref.getattr("_flush_sync")?,))?;
    let signal = py.import("signal")?;
    for name in ["SIGINT", "SIGTERM"] {
        let number = signal.getattr(name)?;
        let previous = signal.call_method1("getsignal", (&number,))?;
        let result = signal.call_method1("signal", (&number, tracker_ref.getattr("_signal_flush")?));
        match result {
            Ok(_) => tracker_ref.borrow().previous_signals.bind(py).set_item(number, previous)?,
            Err(error) if error.is_instance_of::<PyValueError>(py) || error.is_instance_of::<PyOSError>(py) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[pymethods]
impl CostTracker {
    fn _flush_sync(&self, py: Python<'_>) -> PyResult<()> {
        if let Err(error) = self.flush(py) {
            if !error.is_instance_of::<PyException>(py) { return Err(error); }
            error.print(py);
        }
        Ok(())
    }

    fn _signal_flush(&self, py: Python<'_>, signum: i32, frame: &Bound<'_, PyAny>) -> PyResult<()> {
        if self.flushing.load(Ordering::Acquire) {
            let _ = self.pending_signal.compare_exchange(0, signum, Ordering::AcqRel, Ordering::Relaxed);
            return Ok(());
        }
        self._flush_sync(py)?;
        self.forward_signal(py, signum, frame)
    }
}

impl CostTracker {
    pub(super) fn forward_signal(&self, py: Python<'_>, signum: i32, frame: &Bound<'_, PyAny>) -> PyResult<()> {
        let Some(previous) = self.previous_signals.bind(py).get_item(signum)? else { return Ok(()); };
        if previous.is_callable() {
            previous.call1((signum, frame))?;
        } else {
            let signal = py.import("signal")?;
            if previous.eq(signal.getattr("SIG_DFL")?)? {
                signal.call_method1("signal", (signum, previous))?;
                signal.call_method1("raise_signal", (signum,))?;
            }
        }
        Ok(())
    }
}
