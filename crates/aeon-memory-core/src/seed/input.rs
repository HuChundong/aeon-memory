use super::types::*;
use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct LoadAndValidateResult {
    pub input: NormalizedInput,
    pub needs_timestamp_confirmation: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("Seed input validation failed ({count} error(s)):\n{summary}", count = .errors.len(), summary = format_errors(.errors))]
pub struct SeedValidationError {
    pub errors: Vec<ValidationError>,
}

fn format_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(format_validation_error)
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn format_validation_error(e: &ValidationError) -> String {
    let mut parts = vec![format!("  [{:?}]", e.stage).to_lowercase()];
    if let Some(i) = e.source_index {
        parts.push(format!("session[{i}]"));
    }
    if let Some(k) = &e.session_key {
        parts.push(format!("key=\"{k}\""));
    }
    if let Some(i) = e.round_index {
        parts.push(format!("round[{i}]"));
    }
    if let Some(i) = e.message_index {
        parts.push(format!("msg[{i}]"));
    }
    parts.push(e.message.clone());
    parts.join(" ")
}

fn issue(stage: ValidationStage, message: impl Into<String>) -> ValidationError {
    ValidationError {
        stage,
        source_index: None,
        session_key: None,
        round_index: None,
        message_index: None,
        message: message.into(),
    }
}

pub fn load_and_validate_input(
    path: &Path,
    fallback_session_key: Option<&str>,
    strict_round_role: bool,
) -> Result<LoadAndValidateResult, SeedValidationError> {
    let content = std::fs::read_to_string(path).map_err(|e| SeedValidationError {
        errors: vec![issue(
            ValidationStage::File,
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("Input file not found: {}", path.display())
            } else {
                e.to_string()
            },
        )],
    })?;
    if content.trim().is_empty() {
        return Err(SeedValidationError {
            errors: vec![issue(ValidationStage::File, "Input file is empty.")],
        });
    }
    let raw: Value = serde_json::from_str(&content).map_err(|e| SeedValidationError {
        errors: vec![issue(
            ValidationStage::File,
            format!("JSON parse error: {e}"),
        )],
    })?;
    let input = validate_and_normalize_raw(&raw, fallback_session_key, strict_round_role, false)?;
    Ok(LoadAndValidateResult {
        needs_timestamp_confirmation: !input.has_timestamps,
        input,
    })
}

pub fn validate_and_normalize_raw(
    raw: &Value,
    fallback_session_key: Option<&str>,
    strict_round_role: bool,
    auto_fill_timestamps: bool,
) -> Result<NormalizedInput, SeedValidationError> {
    let sessions = if let Some(a) = raw.as_array() {
        a
    } else if let Some(obj) = raw.as_object() {
        match obj.get("sessions") {
            Some(Value::Array(a)) => a,
            Some(_) => {
                return Err(SeedValidationError {
                    errors: vec![issue(
                        ValidationStage::TopLevel,
                        "Format A detected but \"sessions\" is not an array.",
                    )],
                });
            }
            None => return Err(top_level_error()),
        }
    } else {
        return Err(top_level_error());
    };
    let mut errors = Vec::new();
    if sessions.is_empty() {
        errors.push(issue(
            ValidationStage::Session,
            "No sessions found in input.",
        ));
    }
    let mut present = false;
    let mut missing = false;
    for (si, session) in sessions.iter().enumerate() {
        let obj = match session.as_object() {
            Some(v) => v,
            None => {
                let mut e = issue(ValidationStage::Session, "Session must be an object.");
                e.source_index = Some(si);
                errors.push(e);
                continue;
            }
        };
        let key = obj.get("sessionKey").and_then(Value::as_str).unwrap_or("");
        if key.trim().is_empty() {
            push_loc(
                &mut errors,
                ValidationStage::Session,
                si,
                key,
                None,
                None,
                "\"sessionKey\" is required and must be a non-empty string.",
            );
        }
        let rounds = match obj.get("conversations").and_then(Value::as_array) {
            Some(v) => v,
            None => {
                push_loc(
                    &mut errors,
                    ValidationStage::Session,
                    si,
                    key,
                    None,
                    None,
                    "\"conversations\" must be a two-dimensional array (array of rounds).",
                );
                continue;
            }
        };
        for (ri, round) in rounds.iter().enumerate() {
            let messages = match round.as_array() {
                Some(v) if !v.is_empty() => v,
                Some(_) => {
                    push_loc(
                        &mut errors,
                        ValidationStage::Round,
                        si,
                        key,
                        Some(ri),
                        None,
                        "Round must be a non-empty array.",
                    );
                    continue;
                }
                None => {
                    push_loc(
                        &mut errors,
                        ValidationStage::Round,
                        si,
                        key,
                        Some(ri),
                        None,
                        "Round must be an array of messages.",
                    );
                    continue;
                }
            };
            if strict_round_role {
                let roles: Vec<_> = messages
                    .iter()
                    .filter_map(|m| m.get("role").and_then(Value::as_str))
                    .collect();
                if !roles.contains(&"user") {
                    push_loc(
                        &mut errors,
                        ValidationStage::Round,
                        si,
                        key,
                        Some(ri),
                        None,
                        "--strict-round-role: round must contain at least one \"user\" message.",
                    );
                }
                if !roles.contains(&"assistant") {
                    push_loc(
                        &mut errors,
                        ValidationStage::Round,
                        si,
                        key,
                        Some(ri),
                        None,
                        "--strict-round-role: round must contain at least one \"assistant\" message.",
                    );
                }
            }
            for (mi, message) in messages.iter().enumerate() {
                let obj = match message.as_object() {
                    Some(v) => v,
                    None => {
                        push_loc(
                            &mut errors,
                            ValidationStage::Message,
                            si,
                            key,
                            Some(ri),
                            Some(mi),
                            "Message must be an object.",
                        );
                        continue;
                    }
                };
                if obj
                    .get("role")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    push_loc(
                        &mut errors,
                        ValidationStage::Message,
                        si,
                        key,
                        Some(ri),
                        Some(mi),
                        "\"role\" is required and must be a non-empty string.",
                    );
                }
                if obj
                    .get("content")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s.trim().is_empty())
                {
                    push_loc(
                        &mut errors,
                        ValidationStage::Message,
                        si,
                        key,
                        Some(ri),
                        Some(mi),
                        "\"content\" is required and must be a non-empty string.",
                    );
                }
                match obj.get("timestamp") {
                    None | Some(Value::Null) => missing = true,
                    Some(v) => {
                        present = true;
                        if parse_timestamp(v).is_none() {
                            push_loc(
                                &mut errors,
                                ValidationStage::Message,
                                si,
                                key,
                                Some(ri),
                                Some(mi),
                                "\"timestamp\" must be an integer epoch ms or a valid ISO 8601 string.",
                            );
                        }
                    }
                }
            }
        }
    }
    if present && missing {
        errors.push(issue(ValidationStage::TimestampConsistency, "Timestamp consistency check failed: some messages have timestamps while others do not. All messages must either have timestamps or none must have timestamps."));
    }
    if !errors.is_empty() {
        return Err(SeedValidationError { errors });
    }
    let mut normalized = normalize(sessions, fallback_session_key, present);
    if missing && auto_fill_timestamps {
        fill_timestamps(&mut normalized, chrono::Utc::now().timestamp_millis());
    }
    Ok(normalized)
}

fn top_level_error() -> SeedValidationError {
    SeedValidationError {
        errors: vec![issue(
            ValidationStage::TopLevel,
            "Unrecognized input format. Expected either:\n  Format A: { \"sessions\": [...] }\n  Format B: [ { sessionKey, conversations }, ... ]",
        )],
    }
}
fn push_loc(
    errors: &mut Vec<ValidationError>,
    stage: ValidationStage,
    si: usize,
    key: &str,
    ri: Option<usize>,
    mi: Option<usize>,
    message: &str,
) {
    errors.push(ValidationError {
        stage,
        source_index: Some(si),
        session_key: (!key.is_empty()).then(|| key.to_owned()),
        round_index: ri,
        message_index: mi,
        message: message.to_owned(),
    });
}
fn parse_timestamp(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        Some(n)
    } else if let Some(s) = v.as_str() {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis())
    } else {
        None
    }
}

fn normalize(sessions: &[Value], fallback: Option<&str>, has_timestamps: bool) -> NormalizedInput {
    let mut output = Vec::new();
    let mut total_rounds = 0;
    let mut total_messages = 0;
    for (si, value) in sessions.iter().enumerate() {
        let obj = value.as_object().expect("validated");
        let key = obj
            .get("sessionKey")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or(fallback)
            .unwrap_or("seed-user")
            .to_owned();
        let rounds_raw = obj["conversations"].as_array().expect("validated");
        let mut rounds = Vec::new();
        for round in rounds_raw {
            let messages = round
                .as_array()
                .expect("validated")
                .iter()
                .map(|m| {
                    let o = m.as_object().expect("validated");
                    NormalizedMessage {
                        role: o["role"].as_str().unwrap().to_owned(),
                        content: o["content"].as_str().unwrap().to_owned(),
                        timestamp: o.get("timestamp").and_then(parse_timestamp).unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>();
            total_messages += messages.len();
            rounds.push(NormalizedRound { messages });
        }
        total_rounds += rounds.len();
        let session_id = obj
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| deterministic_session_id(si, &key, &rounds));
        output.push(NormalizedSession {
            session_key: key,
            session_id,
            rounds,
            source_index: si,
        });
    }
    NormalizedInput {
        sessions: output,
        total_rounds,
        total_messages,
        has_timestamps,
    }
}

fn deterministic_session_id(index: usize, key: &str, rounds: &[NormalizedRound]) -> String {
    let mut h = Sha256::new();
    h.update(index.to_le_bytes());
    h.update(key.as_bytes());
    h.update(serde_json::to_vec(rounds).expect("serializable"));
    let x = h.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:02x}{:02x}-a{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        x[0],
        x[1],
        x[2],
        x[3],
        x[4],
        x[5],
        x[6] & 15,
        x[7],
        x[8] & 15,
        x[9],
        x[10],
        x[11],
        x[12],
        x[13],
        x[14],
        x[15]
    )
}

pub fn fill_timestamps(input: &mut NormalizedInput, start_ms: i64) {
    let mut ts = start_ms;
    for s in &mut input.sessions {
        for r in &mut s.rounds {
            for m in &mut r.messages {
                m.timestamp = ts;
                ts = ts.saturating_add(100);
            }
        }
    }
    input.has_timestamps = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn accepts_both_shapes_and_generates_stable_ids() {
        let raw = json!({"sessions":[{"sessionKey":"user","conversations":[[{"role":"user","content":"hi"},{"role":"assistant","content":"yo"}]]}]});
        let a = validate_and_normalize_raw(&raw, None, true, false).unwrap();
        let b = validate_and_normalize_raw(&raw, None, true, false).unwrap();
        assert_eq!(a.sessions[0].session_id, b.sessions[0].session_id);
        assert!(!a.has_timestamps);
        assert_eq!(a.total_messages, 2);
    }
    #[test]
    fn rejects_mixed_timestamps_and_collects_locations() {
        let raw = json!([{"sessionKey":"s","conversations":[[{"role":"user","content":"x","timestamp":1},{"role":"assistant","content":""}]]}]);
        let e = validate_and_normalize_raw(&raw, None, false, true).unwrap_err();
        assert!(
            e.errors
                .iter()
                .any(|x| x.stage == ValidationStage::TimestampConsistency)
        );
        assert!(e.errors.iter().any(|x| x.message_index == Some(1)));
    }
    #[test]
    fn auto_fill_is_global_and_monotonic() {
        let raw = json!([{"sessionKey":"a","conversations":[[{"role":"user","content":"x"}]]},{"sessionKey":"b","conversations":[[{"role":"assistant","content":"y"}]]}]);
        let mut i = validate_and_normalize_raw(&raw, None, false, false).unwrap();
        fill_timestamps(&mut i, 1000);
        assert_eq!(i.sessions[0].rounds[0].messages[0].timestamp, 1000);
        assert_eq!(i.sessions[1].rounds[0].messages[0].timestamp, 1100);
    }
}
