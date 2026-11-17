//! Helpers for joining spawned tasks and reporting their panics.

// Shared shutdown stage, also exercised with saturated channels in tests.
// `what` names the tasks in the log line and the failure message.
pub(crate) async fn join_expanders(
    expanders: &mut tokio::task::JoinSet<()>,
    deadline: tokio::time::Instant,
    what: &str,
) -> Option<String> {
    let mut failure = None;
    loop {
        match tokio::time::timeout_at(deadline, expanders.join_next()).await {
            Ok(Some(Err(error))) => failure = Some(format!("{what} failed: {error}")),
            Ok(Some(Ok(()))) => {}
            Ok(None) => break,
            Err(_) => {
                tracing::warn!("{what} exceeded the shutdown grace window; aborting them");
                expanders.shutdown().await;
                break;
            }
        }
    }
    failure
}

/// Format a `JoinHandle` panic payload for logging / error messages.
pub(crate) fn panic_payload_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
