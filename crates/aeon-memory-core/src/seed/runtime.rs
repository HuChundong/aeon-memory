use super::types::*;
use crate::error::AeonMemoryResult;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Instant;

pub struct SeedRound<'a> {
    pub session_key: &'a str,
    pub session_id: &'a str,
    pub round_index: usize,
    pub messages: &'a [NormalizedMessage],
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureOutcome {
    pub l0_recorded_count: usize,
    pub idempotent_skip: bool,
}

#[async_trait]
pub trait SeedRuntime: Send {
    async fn start(&mut self) -> AeonMemoryResult<()>;
    /// Implementations must atomically persist `idempotency_key` with the L0 upserts.
    /// Replays return `idempotent_skip=true` and must not notify the pipeline twice.
    async fn capture_round(&mut self, round: SeedRound<'_>) -> AeonMemoryResult<CaptureOutcome>;
    async fn wait_l1_idle(&mut self, session_keys: &[String]) -> AeonMemoryResult<()>;
    async fn destroy(&mut self) -> AeonMemoryResult<()>;
}

pub async fn execute_seed(
    runtime: &mut dyn SeedRuntime,
    input: &NormalizedInput,
    every_n_conversations: usize,
    output_dir: &Path,
    on_progress: &mut (dyn FnMut(SeedProgress) + Send),
) -> AeonMemoryResult<SeedSummary> {
    let started = Instant::now();
    runtime.start().await?;
    let mut rounds_processed = 0;
    let mut l0 = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let work = async {
        for session in &input.sessions {
            for (ri, round) in session.rounds.iter().enumerate() {
                rounds_processed += 1;
                let key =
                    round_idempotency_key(&session.session_key, &session.session_id, ri, round);
                match runtime
                    .capture_round(SeedRound {
                        session_key: &session.session_key,
                        session_id: &session.session_id,
                        round_index: ri,
                        messages: &round.messages,
                        idempotency_key: key,
                    })
                    .await
                {
                    Ok(o) => {
                        l0 += o.l0_recorded_count;
                        skipped += usize::from(o.idempotent_skip);
                    }
                    Err(_) => failed += 1,
                }
                on_progress(SeedProgress {
                    current_round: rounds_processed,
                    total_rounds: input.total_rounds,
                    session_key: session.session_key.clone(),
                    stage: "l0_captured".to_owned(),
                });
                if every_n_conversations > 0 && (ri + 1) % every_n_conversations == 0 {
                    on_progress(SeedProgress {
                        current_round: rounds_processed,
                        total_rounds: input.total_rounds,
                        session_key: session.session_key.clone(),
                        stage: "l1_waiting".to_owned(),
                    });
                    runtime
                        .wait_l1_idle(std::slice::from_ref(&session.session_key))
                        .await?;
                }
            }
            on_progress(SeedProgress {
                current_round: rounds_processed,
                total_rounds: input.total_rounds,
                session_key: session.session_key.clone(),
                stage: "l1_waiting".to_owned(),
            });
            runtime
                .wait_l1_idle(std::slice::from_ref(&session.session_key))
                .await?;
        }
        let keys = input
            .sessions
            .iter()
            .map(|s| s.session_key.clone())
            .collect::<Vec<_>>();
        runtime.wait_l1_idle(&keys).await
    }
    .await;
    let shutdown = runtime.destroy().await;
    work?;
    shutdown?;
    Ok(SeedSummary {
        sessions_processed: input.sessions.len(),
        rounds_processed,
        messages_processed: input.total_messages,
        l0_recorded_count: l0,
        idempotent_skips: skipped,
        failed_rounds: failed,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        output_dir: output_dir.display().to_string(),
    })
}

pub fn round_idempotency_key(
    session_key: &str,
    session_id: &str,
    round_index: usize,
    round: &NormalizedRound,
) -> String {
    let mut h = Sha256::new();
    h.update(b"aeon-memory-seed-v1\0");
    h.update(session_key.as_bytes());
    h.update([0]);
    h.update(session_id.as_bytes());
    h.update(round_index.to_le_bytes());
    h.update(serde_json::to_vec(round).expect("serializable"));
    crate::lowercase_hex(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[derive(Default)]
    struct Mock {
        seen: HashSet<String>,
        waits: Vec<Vec<String>>,
        destroyed: bool,
    }
    #[async_trait]
    impl SeedRuntime for Mock {
        async fn start(&mut self) -> AeonMemoryResult<()> {
            Ok(())
        }
        async fn capture_round(&mut self, r: SeedRound<'_>) -> AeonMemoryResult<CaptureOutcome> {
            let inserted = self.seen.insert(r.idempotency_key);
            Ok(CaptureOutcome {
                l0_recorded_count: if inserted { r.messages.len() } else { 0 },
                idempotent_skip: !inserted,
            })
        }
        async fn wait_l1_idle(&mut self, k: &[String]) -> AeonMemoryResult<()> {
            self.waits.push(k.to_vec());
            Ok(())
        }
        async fn destroy(&mut self) -> AeonMemoryResult<()> {
            self.destroyed = true;
            Ok(())
        }
    }
    fn input() -> NormalizedInput {
        NormalizedInput {
            sessions: vec![NormalizedSession {
                session_key: "s".into(),
                session_id: "id".into(),
                source_index: 0,
                rounds: (0..3)
                    .map(|i| NormalizedRound {
                        messages: vec![NormalizedMessage {
                            role: "user".into(),
                            content: i.to_string(),
                            timestamp: i,
                        }],
                    })
                    .collect(),
            }],
            total_rounds: 3,
            total_messages: 3,
            has_timestamps: true,
        }
    }
    #[tokio::test]
    async fn batches_waits_and_replay_is_idempotent() {
        let mut rt = Mock::default();
        let mut progress = Vec::new();
        let first = execute_seed(&mut rt, &input(), 2, Path::new("out"), &mut |p| {
            progress.push(p)
        })
        .await
        .unwrap();
        assert_eq!(first.l0_recorded_count, 3);
        assert_eq!(rt.waits.len(), 3);
        assert!(rt.destroyed);
        rt.destroyed = false;
        let second = execute_seed(&mut rt, &input(), 2, Path::new("out"), &mut |_| {})
            .await
            .unwrap();
        assert_eq!(second.l0_recorded_count, 0);
        assert_eq!(second.idempotent_skips, 3);
        assert!(rt.destroyed);
    }
    #[test]
    fn round_idempotency_key_golden() {
        let input = input();
        assert_eq!(
            round_idempotency_key("s", "id", 0, &input.sessions[0].rounds[0]),
            "b9038a6dab297ce183b2cfcf236dfe1d58b1851c572fb79a5db5b07dea724c11"
        );
    }
}
