pub mod commands;
pub mod oracle;
pub mod ownership;
pub mod python;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub supabase: SupabaseFixture,
    pub onboarding: OnboardingFixture,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupabaseFixture {
    pub url: String,
    pub key_env: String,
    pub pagination_records: usize,
    pub pagination_budgets: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnboardingFixture {
    pub url: String,
    pub token_env: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub schema_version: u32,
    pub id: String,
    pub revision: String,
    pub root: PathBuf,
    pub directory: PathBuf,
    pub python: String,
    pub fixture: Fixture,
}

pub fn root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()
        .context("resolve the qualification's owning checkout")
}

pub fn unique_name() -> Result<String> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(format!("{}-{}-{}", elapsed.as_secs(), elapsed.subsec_nanos(), std::process::id()))
}

pub fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file)).with_context(|| format!("decode {}", path.display()))
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() { private_directory(parent)?; }
    let file = OpenOptions::new().write(true).create_new(true).open(path)
        .with_context(|| format!("create evidence without replacing {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

impl Run {
    pub fn load(directory: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        let expected_root = root()?;
        ensure!(directory.starts_with(expected_root.join(".build/spend")), "run directory is outside this checkout's spend evidence");
        let run: Self = read_json(&directory.join("run.json"))?;
        ensure!(run.schema_version == 1 && run.directory == directory && run.root == expected_root,
            "run receipt does not identify this checkout and directory");
        Ok(run)
    }

    pub fn result(&self, name: &str, value: &Value) -> Result<()> {
        write_json(&self.directory.join("results").join(format!("{name}.json")), value)
    }

    pub fn state_path(&self, case: &str) -> PathBuf {
        self.directory.join("state").join(format!("{case}.json"))
    }

    pub fn site(&self) -> PathBuf { self.directory.join("python-site") }

    pub fn environment(&self, case: &str, central: bool) -> Result<Vec<(String, String)>> {
        let scratch = self.directory.join("scratch");
        private_directory(&scratch)?;
        let mut values = vec![
            ("PYTHONPATH".into(), self.site().to_string_lossy().into_owned()),
            ("PYTHONNOUSERSITE".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("TMPDIR".into(), scratch.to_string_lossy().into_owned()),
            ("TMP".into(), scratch.to_string_lossy().into_owned()),
            ("TEMP".into(), scratch.to_string_lossy().into_owned()),
            ("PIP_CACHE_DIR".into(), self.directory.join("cache/pip").to_string_lossy().into_owned()),
            ("CARGO_TARGET_DIR".into(), self.directory.join("native-target").to_string_lossy().into_owned()),
            ("PYO3_PYTHON".into(), self.python.clone()),
            ("WISENT_COST_TRACKER_ONBOARDING_STATE_PATH".into(), self.state_path(case).to_string_lossy().into_owned()),
            ("STADO_INTEGRATION_API_URL".into(), String::new()),
            ("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN".into(), String::new()),
        ];
        if central {
            values.push(("STADO_INTEGRATION_API_URL".into(), self.fixture.onboarding.url.clone()));
            values.push(("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN".into(), credential(&self.fixture.onboarding.token_env)?));
        }
        Ok(values)
    }
}

pub fn credential(name: &str) -> Result<String> {
    let value = std::env::var(name).with_context(|| format!("required real-service credential {name} is unavailable"))?;
    ensure!(!value.trim().is_empty(), "required real-service credential {name} is empty");
    Ok(value)
}
