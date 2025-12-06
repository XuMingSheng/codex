//! Session-wide mutable state.

use crate::codex::SessionConfiguration;
use crate::protocol::RateLimitSnapshot;

/// Persistent, session-scoped state previously stored directly on `Session`.
pub(crate) struct SessionState {
    pub(crate) session_configuration: SessionConfiguration,
    pub(crate) latest_rate_limits: Option<RateLimitSnapshot>,
}

impl SessionState {
    /// Create a new session state mirroring previous `State::default()` semantics.
    pub(crate) fn new(session_configuration: SessionConfiguration) -> Self {
        Self {
            session_configuration,
            latest_rate_limits: None,
        }
    }

    pub(crate) fn set_rate_limits(&mut self, snapshot: RateLimitSnapshot) {
        self.latest_rate_limits = Some(snapshot);
    }

    pub(crate) fn rate_limits(&self) -> Option<RateLimitSnapshot> {
        self.latest_rate_limits.clone()
    }
}
