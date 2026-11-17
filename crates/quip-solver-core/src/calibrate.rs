//! The `Calibrate` exchange: run the sampler's calibrate hook, enforce the
//! deadline, and answer with exactly one `CalibrateDone`.

use crate::coefficient::Coefficient;
use crate::job::miner;
use crate::pipeline::Control;
use crate::Sampler;
use quip_proto::v1::{miner_msg, Calibrate, CalibrateDone, CalibrateOutcome, Topology};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::Instant;

/// The tuning change a [`Sampler::calibrate`] call found, and a summary for
/// the operator.
///
/// The session runs `apply` only when the call returned before the deadline.
/// Keep it short: it runs on a blocking thread after the deadline check.
pub struct Calibration<S: ?Sized> {
    summary: String,
    apply: Box<dyn FnOnce(&S) + Send>,
}

impl<S: ?Sized> Calibration<S> {
    /// A calibration that reports `summary` (empty for none) and changes the
    /// tuning with `apply`.
    pub fn new(summary: impl Into<String>, apply: impl FnOnce(&S) + Send + 'static) -> Self {
        Self {
            summary: summary.into(),
            apply: Box::new(apply),
        }
    }
}

fn reply(outcome: CalibrateOutcome, summary: impl Into<String>) -> CalibrateDone {
    let summary = summary.into();
    CalibrateDone {
        outcome: outcome as i32,
        summary: (!summary.is_empty()).then_some(summary),
    }
}

/// The deadline of `deadline_ms` (Unix milliseconds, 0 for none) on the
/// runtime clock. `Err` when it has already passed, or when the system clock
/// cannot tell, which fails closed as an expired job does.
fn deadline_instant(deadline_ms: u64) -> Result<Option<Instant>, ()> {
    if deadline_ms == 0 {
        return Ok(None);
    }
    let now_ms = crate::job::now_unix_ms().ok_or(())?;
    if deadline_ms <= now_ms {
        return Err(());
    }
    Ok(Some(
        Instant::now() + Duration::from_millis(deadline_ms - now_ms),
    ))
}

/// Answer one `Calibrate`. Every path produces exactly one `CalibrateDone`.
///
/// The hook runs on a blocking thread while holding one of `permits`, so a
/// second `Calibrate` waits for the first call to return, even one that
/// overran its deadline. The deadline covers that wait. The tuning changes
/// only when the hook returned `Ok` before the deadline.
pub(crate) async fn answer<S: Sampler<C>, C: Coefficient>(
    sampler: Arc<S>,
    request: Calibrate,
    topology: Option<Arc<Topology>>,
    permits: Arc<Semaphore>,
) -> CalibrateDone {
    if !S::accepts_calibrate() {
        return reply(
            CalibrateOutcome::Failed,
            "this solver does not list the calibrate feature",
        );
    }
    let Some(topology) = topology else {
        return reply(
            CalibrateOutcome::Failed,
            "no Topology arrived before Calibrate",
        );
    };
    let Ok(deadline) = deadline_instant(request.deadline_ms) else {
        return reply(
            CalibrateOutcome::DeadlinePassed,
            "the deadline had passed on arrival",
        );
    };
    let worker = Arc::clone(&sampler);
    let call = async move {
        let Ok(permit) = permits.acquire_owned().await else {
            return Err("the session is closing".to_owned());
        };
        tokio::task::spawn_blocking(move || {
            // Held until the hook returns, even after the deadline answer.
            let _permit = permit;
            worker.calibrate(&request, &topology)
        })
        .await
        .map_err(|error| format!("calibrate panicked: {error}"))?
        .map_err(|error| error.to_string())
    };
    let outcome = match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, call).await,
        None => Ok(call.await),
    };
    match outcome {
        Err(_elapsed) => reply(
            CalibrateOutcome::DeadlinePassed,
            "calibration was still running at the deadline",
        ),
        Ok(Err(detail)) => {
            tracing::warn!(detail, "calibration failed; keeping the current tuning");
            reply(CalibrateOutcome::Failed, detail)
        }
        Ok(Ok(calibration)) => {
            let Calibration { summary, apply } = calibration;
            match tokio::task::spawn_blocking(move || apply(&*sampler)).await {
                Ok(()) => reply(CalibrateOutcome::Done, summary),
                Err(error) => reply(
                    CalibrateOutcome::Failed,
                    format!("applying the calibration panicked: {error}"),
                ),
            }
        }
    }
}

/// Answer one `Calibrate` and send the reply. Spawned by the session read
/// loop, so a long calibration never blocks `Ping` or `Shutdown`.
pub(crate) async fn run<S: Sampler<C>, C: Coefficient>(
    sampler: Arc<S>,
    request: Calibrate,
    topology: Option<Arc<Topology>>,
    permits: Arc<Semaphore>,
    ctrl: mpsc::Sender<Control>,
) {
    let done = answer(sampler, request, topology, permits).await;
    let _ = ctrl
        .send(miner(miner_msg::Msg::CalibrateDone(done)).into())
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coefficient::Milli;
    use crate::{IsingGraph, SampleError, SampleParams, SamplerResult};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// How a test sampler's calibrate hook behaves.
    #[derive(Clone, Copy)]
    enum Hook {
        /// Return a calibration that sets the tuning to 7.
        Tune,
        /// Return an error.
        Fail,
        /// Sleep this long, then return a calibration that sets the tuning
        /// to 7.
        Slow(Duration),
        /// Panic.
        Panic,
    }

    struct Tuned {
        hook: Hook,
        tuning: AtomicU32,
        called: AtomicBool,
    }

    impl Tuned {
        fn new(hook: Hook) -> Arc<Self> {
            Arc::new(Self {
                hook,
                tuning: AtomicU32::new(1),
                called: AtomicBool::new(false),
            })
        }
    }

    impl Sampler<Milli> for Tuned {
        fn sample(
            &self,
            _: &IsingGraph<Milli>,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(Vec::new())
        }
        fn accepts_calibrate() -> bool {
            true
        }
        #[expect(
            clippy::panic_in_result_fn,
            reason = "Hook::Panic exercises the session's handling of a panicking hook"
        )]
        fn calibrate(
            &self,
            request: &Calibrate,
            topology: &Topology,
        ) -> Result<Calibration<Self>, SampleError> {
            self.called.store(true, Ordering::SeqCst);
            if request.instances.len() != 1 || topology.nodes != [3, 5] {
                return Err(SampleError::Declined("the hook got another request".into()));
            }
            let tuned = Calibration::new("beta 7", |s: &Self| s.tuning.store(7, Ordering::SeqCst));
            match self.hook {
                Hook::Tune => Ok(tuned),
                Hook::Fail => Err(SampleError::Declined("no better tuning".into())),
                Hook::Slow(delay) => {
                    std::thread::sleep(delay);
                    Ok(tuned)
                }
                Hook::Panic => panic!("calibrate hook panicked"),
            }
        }
    }

    /// A sampler that keeps the default, unsupported hook.
    struct Untuned;
    impl Sampler<Milli> for Untuned {
        fn sample(
            &self,
            _: &IsingGraph<Milli>,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(Vec::new())
        }
    }

    fn now_ms() -> u64 {
        crate::job::now_unix_ms().unwrap()
    }

    fn request(deadline_ms: u64) -> Calibrate {
        Calibrate {
            family: None,
            instances: vec![quip_proto::v1::CalibrationInstance {
                id: "ref-0".into(),
                nonce: vec![9; 32],
                achieved_energy_milli: -1200,
                max_energy_milli: -1000,
            }],
            deadline_ms,
        }
    }

    fn topology() -> Arc<Topology> {
        Arc::new(Topology {
            nodes: vec![3, 5],
            ..Topology::default()
        })
    }

    fn permits() -> Arc<Semaphore> {
        Arc::new(Semaphore::new(1))
    }

    fn outcome(done: &CalibrateDone) -> CalibrateOutcome {
        CalibrateOutcome::try_from(done.outcome).unwrap()
    }

    #[tokio::test]
    async fn a_calibration_inside_the_deadline_is_applied_and_reported_done() {
        let sampler = Tuned::new(Hook::Tune);
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 5_000),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Done);
        assert_eq!(done.summary.as_deref(), Some("beta 7"));
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn no_deadline_waits_for_the_hook() {
        let sampler = Tuned::new(Hook::Tune);
        let done = answer(
            Arc::clone(&sampler),
            request(0),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Done);
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn a_failed_hook_keeps_the_tuning_and_reports_why() {
        let sampler = Tuned::new(Hook::Fail);
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 5_000),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Failed);
        assert_eq!(done.summary.as_deref(), Some("declined: no better tuning"));
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_panicking_hook_keeps_the_tuning_and_fails() {
        let sampler = Tuned::new(Hook::Panic);
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 5_000),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Failed);
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_hook_past_the_deadline_is_answered_at_the_deadline_and_never_applied() {
        let sampler = Tuned::new(Hook::Slow(Duration::from_millis(600)));
        let started = std::time::Instant::now();
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 150),
            Some(topology()),
            permits(),
        )
        .await;
        let answered_after = started.elapsed();
        assert_eq!(outcome(&done), CalibrateOutcome::DeadlinePassed);
        assert!(
            answered_after < Duration::from_millis(500),
            "answered {answered_after:?} after the request, not at the deadline"
        );
        // The hook finishes after the deadline; its change must still be dropped.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(sampler.called.load(Ordering::SeqCst));
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_deadline_already_passed_does_not_call_the_hook() {
        let sampler = Tuned::new(Hook::Tune);
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() - 1_000),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::DeadlinePassed);
        assert!(!sampler.called.load(Ordering::SeqCst));
        assert_eq!(sampler.tuning.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_deadline_covers_the_wait_for_an_earlier_calibration() {
        let sampler = Tuned::new(Hook::Tune);
        let permits = permits();
        let held = Arc::clone(&permits).acquire_owned().await.unwrap();
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 100),
            Some(topology()),
            permits,
        )
        .await;
        drop(held);
        assert_eq!(outcome(&done), CalibrateOutcome::DeadlinePassed);
        assert!(!sampler.called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_calibrate_before_any_topology_fails_without_calling_the_hook() {
        let sampler = Tuned::new(Hook::Tune);
        let done = answer(
            Arc::clone(&sampler),
            request(now_ms() + 5_000),
            None,
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Failed);
        assert!(!sampler.called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_solver_without_the_feature_answers_failed() {
        let done = answer(
            Arc::new(Untuned),
            request(now_ms() + 5_000),
            Some(topology()),
            permits(),
        )
        .await;
        assert_eq!(outcome(&done), CalibrateOutcome::Failed);
        assert!(done.summary.is_some_and(|s| s.contains("calibrate")));
    }
}
