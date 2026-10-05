mod file;
mod memory;
mod supabase;

use pyo3::prelude::*;

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<file::FileSink>()?;
    module.add_class::<memory::MemorySink>()?;
    module.add_class::<supabase::SupabaseSink>()?;
    Ok(())
}
