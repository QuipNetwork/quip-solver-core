//! The channels between the session read loop, the sampler, and the outbound
//! writer, and the per-job state the writer finalizes.

use crate::coefficient::Coefficient;
use crate::families::FamilyContext;
use crate::job::{miner, ExactEnergy};
#[cfg(doc)]
use crate::Sampler;
use crate::{CancelToken, StreamJob, WarmStart, WarmStreamJob};
use quip_proto::v1::{miner_msg, JobRequest, MinerMsg};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::mpsc;

/// The sending half of the channel into the sampler thread. Which one depends
/// on [`Sampler::accepts_warm_start`] and [`Sampler::accepts_family_metadata`],
/// fixed for the session.
#[derive(Clone)]
pub(crate) enum JobSender<C: Coefficient> {
    /// Feeds [`Sampler::sample_stream`]; warm starts and family contexts are
    /// dropped.
    Plain(mpsc::Sender<StreamJob<C>>),
    /// Feeds [`Sampler::sample_stream_warm`].
    Warm(mpsc::Sender<WarmStreamJob<C>>),
}

impl<C: Coefficient> JobSender<C> {
    /// Send one job. `Err` means the sampler thread is gone.
    pub(crate) async fn send(
        &self,
        job: StreamJob<C>,
        warm_start: Option<WarmStart>,
        family: Option<FamilyContext>,
    ) -> Result<(), ()> {
        match self {
            Self::Plain(tx) => tx.send(job).await.map_err(|_| ()),
            Self::Warm(tx) => tx
                .send(WarmStreamJob {
                    job,
                    warm_start,
                    family,
                })
                .await
                .map_err(|_| ()),
        }
    }
}

/// What the read loop knew about a job at prepare time, held until the writer
/// finalizes it.
///
/// The sampling parameters build the outbound `SamplerMeta`. The rest exists so
/// the completion log can report an attempt the way the v0.2.1 miner did:
/// elapsed wall time, and the requirement the attempt was measured against.
/// Plain jobs retain their admission target for logging. Lease results carry
/// no target: the writer reports every completed salt's reads unfiltered.
pub(crate) struct PendingJob<L> {
    pub(crate) lease: Option<L>,
    /// Original wire graph, present only for lossy coefficient types.
    pub(crate) exact_energy: Option<ExactEnergy>,
    /// The instance offset of a family lease salt, added after rescoring.
    /// Zero for every other job.
    pub(crate) offset_milli: i64,
    pub(crate) num_reads: u32,
    pub(crate) num_sweeps: u32,
    pub(crate) started: std::time::Instant,
    /// Session energy threshold, or `None` when no `SetTarget` has arrived.
    pub(crate) max_energy_milli: Option<i64>,
    pub(crate) min_solutions: u32,
    /// This job's cancellation watermark, as `prepare_job` resolved it. The
    /// writer re-checks it so a `Result` for a generation the coordinator
    /// abandoned cannot reach the wire, whatever the backend decided.
    pub(crate) watermark: Option<u64>,
}

/// `job_id` → its [`PendingJob`], shared between the session's read loop (which
/// inserts) and its outbound writer (which removes).
pub(crate) type PendingParams<L> = Arc<StdMutex<HashMap<Vec<u8>, PendingJob<L>>>>;

pub(crate) fn pending_id(wire_id: &[u8], salt_index: Option<u64>) -> Vec<u8> {
    // Internal IDs are 0 || plain wire ID or 1 || lease wire ID || index LE64.
    // Distinct tags make the namespaces disjoint for arbitrary coordinator IDs.
    // Stream jobs, completions, and the pending map use them internally. The writer
    // removes the tag and salt suffix after lookup, before cancellation, rescoring,
    // logging, and wire replies.
    let mut id = Vec::with_capacity(1 + wire_id.len() + salt_index.map_or(0, |_| 8));
    id.push(u8::from(salt_index.is_some()));
    id.extend_from_slice(wire_id);
    if let Some(index) = salt_index {
        id.extend_from_slice(&index.to_le_bytes());
    }
    id
}

/// The stop state a local message carries to the writer, checked when the
/// writer commits it.
pub(crate) trait LocalDelivery: Send + Sync {
    /// Send `msgs` on the reserved `permits`, dropping each `Result` that must
    /// not reach the wire because the work stopped or `close_expired` is set.
    fn send_local(
        &self,
        msgs: Vec<MinerMsg>,
        permits: mpsc::PermitIterator<'_, MinerMsg>,
        cancel: &CancelToken,
        close_expired: bool,
    );
    /// Whether the close deadline has passed.
    fn close_expired(&self) -> bool;
}

/// Local messages retain their stop state until the writer commits them.
pub(crate) struct Control {
    msg: MinerMsg,
    /// Send `JobRequest { credits: 1 }` right after `msg`, from the same reservation.
    refund: bool,
    local: Option<Box<dyn LocalDelivery>>,
}
impl From<MinerMsg> for Control {
    fn from(msg: MinerMsg) -> Self {
        Self {
            msg,
            refund: false,
            local: None,
        }
    }
}
impl Control {
    pub(crate) fn local(msg: MinerMsg, local: Box<dyn LocalDelivery>) -> Self {
        Self {
            msg,
            refund: false,
            local: Some(local),
        }
    }
    /// A lease summary and its credit refund. The writer commits both or neither.
    pub(crate) fn summary(done: MinerMsg, local: Option<Box<dyn LocalDelivery>>) -> Self {
        Self {
            msg: done,
            refund: true,
            local,
        }
    }
}

pub(crate) async fn send_control(
    tx: &mpsc::Sender<MinerMsg>,
    control: Control,
    cancel: &CancelToken,
) -> bool {
    let count = if control.refund { 2 } else { 1 };
    // Ready terminal messages must not yield on a depleted task budget at
    // the grace boundary. Only wait when the channel is actually full.
    let permits = match tx.try_reserve_many(count) {
        Ok(permits) => permits,
        Err(mpsc::error::TrySendError::Closed(())) => return false,
        Err(mpsc::error::TrySendError::Full(())) => {
            let Ok(permits) = tx.reserve_many(count).await else {
                return false;
            };
            permits
        }
    };
    let fatal = matches!(control.msg.msg, Some(miner_msg::Msg::Fatal(_)));
    let mut msgs = vec![control.msg];
    if control.refund {
        msgs.push(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })));
    }
    if let Some(local) = &control.local {
        let close_expired = local.close_expired();
        local.send_local(msgs, permits, cancel, close_expired);
        return true;
    }
    for (permit, msg) in permits.zip(msgs) {
        permit.send(msg);
    }
    !fatal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_control_uses_available_capacity_with_exhausted_task_budget() {
        use std::future::Future as _;
        let (tx, mut rx) = mpsc::channel(2);
        let cancel = CancelToken::default();
        let message = miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        // Consume the current task budget without yielding to another task.
        while std::pin::pin!(tokio::task::consume_budget())
            .poll(&mut context)
            .is_ready()
        {}
        assert!(
            std::pin::pin!(send_control(&tx, message.into(), &cancel))
                .poll(&mut context)
                .is_ready(),
            "ready terminal send yielded past the grace deadline"
        );
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn a_summary_and_its_refund_are_committed_together() {
        use std::future::Future as _;
        let (tx, mut rx) = mpsc::channel(2);
        let cancel = CancelToken::default();
        tx.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 7 })))
            .await
            .unwrap();
        let done = miner(miner_msg::Msg::LeaseDone(quip_proto::v1::LeaseDone {
            job_id: b"lease".to_vec(),
            salts_done: 3,
            best_energy_milli: -5,
        }));
        let send = send_control(&tx, Control::summary(done.clone(), None), &cancel);
        tokio::pin!(send);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            send.as_mut().poll(&mut context).is_pending(),
            "one free slot must not take half a summary"
        );
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 7 }))
        );
        assert!(rx.try_recv().is_err());
        assert!(send.as_mut().poll(&mut context).is_ready());
        assert_eq!(rx.try_recv().unwrap(), done);
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))
        );
    }
}
