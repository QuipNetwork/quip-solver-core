//! Static backend identity and the feature strings a backend advertises.

use quip_proto::v1::{Algorithm, Backend};

/// Static backend metadata advertised in capabilities and Hello.
#[derive(Clone, Copy, Debug)]
pub struct BackendIdentity {
    /// Backend name advertised in Hello / capabilities (e.g. `"cpu"`, `"cuda"`).
    pub backend: Backend,
    /// Algorithm name advertised in Hello / capabilities (e.g. `"sa"`, `"gibbs"`).
    pub algorithm: Algorithm,
    /// Hard cap on variables accepted for a job.
    pub max_nodes: u32,
    /// Hard cap on edges accepted for a job.
    pub max_edges: u32,
    /// Extra capability names advertised in `Hello` and `Capabilities`
    /// (for example `"streaming"`, `"governor"`). Empty for a plain backend.
    pub features: &'static [&'static str],
    /// Sampling-parameter envelope used by `adapt::adapt_params`.
    pub adapt: crate::adapt::AdaptBounds,
    /// Which models this backend solves, beyond `max_nodes` and `max_edges`.
    /// Advertised in `Capabilities` and enforced on every plain job and every
    /// `--solve` problem. [`ModelLimits::UNSTATED`](crate::ModelLimits::UNSTATED)
    /// makes no claim.
    pub limits: crate::limits::ModelLimits,
}

/// Feature string for a solver whose ladder honours
/// [`BetaSchedule::Linear`](crate::BetaSchedule::Linear). A binary lists it in
/// `BackendIdentity.features`; `--solve` refuses a linear schedule otherwise.
pub const LINEAR_SCHEDULE_FEATURE: &str = "linear-schedule";

/// Feature string for the refuse-not-quantize policy. A binary that lists it
/// gets every model refused, as capacity, that it would not represent exactly:
/// a coefficient off its advertised exact resolution, or one its coefficient
/// type would round or saturate. A binary that does not list it keeps
/// quantizing, and the SDK rescores the energies.
pub const EXACT_COEFFICIENTS_FEATURE: &str = "exact-coefficients";
