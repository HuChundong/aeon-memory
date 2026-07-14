pub fn is_non_interactive_trigger(trigger: Option<&str>, key: Option<&str>) -> bool {
    trigger.is_some_and(|t| {
        matches!(
            t.to_ascii_lowercase().as_str(),
            "cron" | "heartbeat" | "automation" | "schedule"
        )
    }) || key.is_some_and(|k| {
        let k = k.to_ascii_lowercase();
        k.contains(":cron:") || k.contains(":heartbeat:")
    })
}
fn glob_match(pattern: &str, key: &str) -> bool {
    let parts = pattern.split('*').collect::<Vec<_>>();
    let mut pos = 0;
    for p in parts {
        if p.is_empty() {
            continue;
        }
        let Some(found) = key[pos..].find(p) else {
            return false;
        };
        pos += found + p.len()
    }
    true
}
#[derive(Clone, Debug)]
pub struct SessionFilter {
    patterns: Vec<String>,
}
impl SessionFilter {
    pub fn new(patterns: &[String]) -> Self {
        Self {
            patterns: patterns
                .iter()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
    pub fn should_skip(&self, key: &str) -> bool {
        key.contains(":memory-scene-extract-")
            || key.contains(":subagent:")
            || key.starts_with("temp:")
            || self.patterns.iter().any(|p| glob_match(p, key))
    }
    pub fn should_skip_ctx(
        &self,
        key: Option<&str>,
        id: Option<&str>,
        trigger: Option<&str>,
    ) -> bool {
        let Some(key) = key else { return true };
        id.is_some_and(|x| x.starts_with("memory-"))
            || is_non_interactive_trigger(trigger, Some(key))
            || self.should_skip(key)
    }
}
