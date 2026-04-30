use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub language: Option<String>,
}

pub fn load(repo_root: &Path) -> Result<ProjectConfig> {
    let path = repo_root.join("kobito.toml");
    if !path.exists() {
        return Ok(ProjectConfig::default());
    }
    let body = fs::read_to_string(&path)?;
    Ok(toml::from_str(&body)?)
}

pub fn resolve_language(cli_arg: Option<&str>, project: &ProjectConfig) -> String {
    cli_arg
        .map(|s| s.to_string())
        .or_else(|| project.language.clone())
        .unwrap_or_else(|| "en".to_string())
}
