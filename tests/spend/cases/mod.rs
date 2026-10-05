pub mod lifecycle;
mod local;
mod onboarding;
mod supabase;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::support::{Run, credential};

/// The operation field in the qualification driver's worker protocol.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Case { Local, Supabase, Onboarding, Lifecycle }

impl Case {
    pub const ALL: [Self; 4] = [Self::Local, Self::Supabase, Self::Onboarding, Self::Lifecycle];

    pub fn name(self) -> Result<String> {
        match serde_json::to_value(self)? {
            Value::String(name) => Ok(name),
            _ => anyhow::bail!("worker operation did not serialize as its declared enum"),
        }
    }

    pub fn prerequisites(self, run: &Run) -> Result<()> {
        match self {
            Self::Supabase => { credential(&run.fixture.supabase.key_env)?; }
            Self::Onboarding => { credential(&run.fixture.onboarding.token_env)?; }
            Self::Local | Self::Lifecycle => (),
        }
        Ok(())
    }

    pub fn execute(self, run: &Run) -> Result<Value> {
        match self {
            Self::Local => local::run(run),
            Self::Supabase => supabase::run(run),
            Self::Onboarding => onboarding::run(run),
            Self::Lifecycle => lifecycle::run(run),
        }
    }
}
