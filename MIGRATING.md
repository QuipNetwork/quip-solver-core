# Migrating to quip-solver-core 0.0.0-rc1

This note is for maintainers of `quip-miner-cpu` and `quip-miner-cuda`.
Two public types change in this release. A build that still names the old
types fails.

## Dependency

The crate moved. Change the git source and the package name.

Before:

```toml
[dependencies]
quip-miner-core = { git = "https://gitlab.com/quip.network/quip-protocol.git", tag = "<tag>" }
```

After:

```toml
[dependencies]
quip-solver-core = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "<tag>" }
```

Replace `<tag>` with the tag chosen at publication.

## `Sampler::sample` error type

`sample` no longer returns a wire `RejectReason`. It returns a
`SampleError`. The session maps `SampleError` to the wire at the boundary.
A `Sampler` reports a device condition. It does not name wire enum values.

Before:

```rust
fn sample(
    &self,
    graph: &IsingGraph,
    params: &SampleParams,
) -> Result<Vec<SamplerResult>, RejectReason>;
```

After:

```rust
fn sample(
    &self,
    graph: &IsingGraph,
    params: &SampleParams,
) -> Result<Vec<SamplerResult>, SampleError>;
```

Map a size bound to `Capacity`. Map transient load to `DeviceBusy`. Map a
state that needs a restart to `DeviceFault`.

## Cancellation

This release removes `CancelGuard` and `StreamJob.generation`. Use
`CancelToken` and `StreamJob.watermark: Option<u64>`. A `Cancel` never
abandons a job with no watermark.

Before:

```rust
pub struct StreamJob {
    pub job_id: Vec<u8>,
    pub graph: IsingGraph,
    pub params: SampleParams,
    pub generation: u64,
}

fn sample_stream(
    &self,
    jobs: tokio::sync::mpsc::Receiver<StreamJob>,
    out: tokio::sync::mpsc::Sender<StreamResult>,
    cancel: CancelGuard,
);

impl CancelGuard {
    pub fn is_cancelled(&self, generation: u64) -> bool;
}
```

After:

```rust
pub struct StreamJob {
    pub job_id: Vec<u8>,
    pub graph: IsingGraph,
    pub params: SampleParams,
    pub watermark: Option<u64>,
}

fn sample_stream(
    &self,
    jobs: tokio::sync::mpsc::Receiver<StreamJob>,
    out: tokio::sync::mpsc::Sender<StreamResult>,
    cancel: CancelToken,
);

impl CancelToken {
    pub fn is_cancelled(&self, watermark: Option<u64>) -> bool;
}
```

Generation `0` used to mean a mempool job that a reseed never cancels.
That rule now lives in `prepare_job`. The token itself has no zero case.
Pass `watermark: None` for a job that `Cancel` must not abandon.

This note does not claim that any consumer repository has migrated.
