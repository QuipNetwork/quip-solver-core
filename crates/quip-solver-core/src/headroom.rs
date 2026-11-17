//! The budget headroom a solver reports in each `Status`.

/// How much of its spending or capacity budget a solver has left, as
/// [`Sampler::headroom`](crate::Sampler::headroom) returns it.
///
/// The session copies both fields into every `Status` it sends, without
/// change. It does not clamp `fraction`: a value outside `0.0..=1.0` reaches
/// the wire, and the conformance driver reports it as a violation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Headroom {
    /// The fraction of the budget still available, `0.0` to `1.0`. Sent as
    /// `Status.headroom`.
    pub fraction: f64,
    /// The solver has hit a limit. It must refuse new jobs with `OVERLOADED`
    /// until the limit resets. Sent as `Status.limit_reached`. The session
    /// does not refuse jobs for the solver: `sample` returns
    /// [`SampleError::DeviceBusy`](crate::SampleError::DeviceBusy) while the
    /// limit holds.
    pub limit_reached: bool,
}
