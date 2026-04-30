use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub fn config_root() -> PathBuf {
    if let Ok(custom) = std::env::var("XDG_CONFIG_HOME")
        && !custom.is_empty()
    {
        return PathBuf::from(custom).join("kobito");
    }
    dirs::home_dir()
        .map(|h| h.join(".config/kobito"))
        .unwrap_or_else(|| PathBuf::from(".kobito-config"))
}

pub fn resolve(name: &str, repo: &Path) -> Result<PathBuf> {
    let local = repo.join(".kobito/presets").join(format!("{name}.md"));
    if local.exists() {
        return Ok(local);
    }
    let global = config_root().join("presets").join(format!("{name}.md"));
    if global.exists() {
        return Ok(global);
    }
    bail!(
        "preset `{name}` not found — looked in {} and {}",
        local.display(),
        global.display()
    )
}

pub fn load(name: &str, repo: &Path, vars: &HashMap<String, String>) -> Result<String> {
    let path = resolve(name, repo)?;
    let body = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    substitute(&body, vars)
}

pub fn substitute(body: &str, vars: &HashMap<String, String>) -> Result<String> {
    let re = pattern();
    let mut missing: Vec<String> = Vec::new();
    let result = re.replace_all(body, |caps: &regex::Captures| {
        let key = caps[1].trim();
        match vars.get(key) {
            Some(v) => v.clone(),
            None => {
                missing.push(key.to_string());
                String::new()
            }
        }
    });
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        bail!(
            "preset has unresolved variable(s): {} — pass --var {}=<value>",
            missing.join(", "),
            missing[0]
        );
    }
    Ok(result.into_owned())
}

pub fn parse_vars(raw: &[String]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for s in raw {
        match s.split_once('=') {
            Some((k, v)) => {
                map.insert(k.trim().to_string(), v.to_string());
            }
            None => bail!("--var expected key=value, got `{s}`"),
        }
    }
    Ok(map)
}

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\}\}").unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitute_replaces_variables() {
        let v = vars(&[("path", "src/api"), ("target", "80")]);
        let out = substitute("Cover {{path}} to {{target}}%", &v).unwrap();
        assert_eq!(out, "Cover src/api to 80%");
    }

    #[test]
    fn substitute_tolerates_whitespace_in_braces() {
        let v = vars(&[("name", "kobito")]);
        let out = substitute("Hello {{ name }}", &v).unwrap();
        assert_eq!(out, "Hello kobito");
    }

    #[test]
    fn substitute_errors_on_missing_var() {
        let err = substitute("Cover {{path}}", &HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("path"));
    }

    #[test]
    fn substitute_reports_all_missing_vars() {
        let err = substitute("{{a}} and {{b}}", &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("a"));
        assert!(msg.contains("b"));
    }

    #[test]
    fn parse_vars_accepts_key_value() {
        let raw = vec!["a=1".into(), "b=foo bar".into()];
        let map = parse_vars(&raw).unwrap();
        assert_eq!(map.get("a"), Some(&"1".to_string()));
        assert_eq!(map.get("b"), Some(&"foo bar".to_string()));
    }

    #[test]
    fn parse_vars_handles_equals_in_value() {
        let raw = vec!["expr=x=y+1".into()];
        let map = parse_vars(&raw).unwrap();
        assert_eq!(map.get("expr"), Some(&"x=y+1".to_string()));
    }

    #[test]
    fn parse_vars_rejects_no_equals() {
        let raw = vec!["bad".into()];
        assert!(parse_vars(&raw).is_err());
    }
}
