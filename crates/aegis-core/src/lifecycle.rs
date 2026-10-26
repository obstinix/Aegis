//! VM lifecycle state machine (ADR-0007).
//!
//! ```text
//! Created --Configure--> Configured --Start--> Running <--Pause/Resume--> Paused
//!                                                 |                          |
//!                                                 +-----------Stop-----------+--> Stopped --Destroy--> Destroyed
//! Fail is accepted from Configured, Running and Paused and leads to Failed; Failed --Destroy--> Destroyed.
//! ```
//!
//! Invalid transitions return an error and leave the state unchanged. They
//! never panic.

use core::fmt;

/// The state a VM is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmState {
    /// Allocated, no configuration applied yet.
    Created,
    /// Configuration validated and applied; no vCPU has run.
    Configured,
    /// At least one vCPU may be executing guest code.
    Running,
    /// Execution suspended; resumable.
    Paused,
    /// Execution ended normally; resources not yet released.
    Stopped,
    /// Execution ended because of an error; resources not yet released.
    Failed,
    /// All resources released. Terminal.
    Destroyed,
}

/// An event that requests a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmEvent {
    /// Apply a validated configuration.
    Configure,
    /// Begin executing the guest.
    Start,
    /// Suspend execution.
    Pause,
    /// Continue after a pause.
    Resume,
    /// End execution normally.
    Stop,
    /// End execution because of an error.
    Fail,
    /// Release all resources.
    Destroy,
}

/// A transition that is not allowed from the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleError {
    /// State the VM was in when the event arrived.
    pub from: VmState,
    /// The rejected event.
    pub event: VmEvent,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "event {:?} is not valid in state {:?}",
            self.event, self.from
        )
    }
}

impl core::error::Error for LifecycleError {}

impl VmState {
    /// The state reached by applying `event`, or `None` if the transition is
    /// not allowed.
    #[must_use]
    pub const fn next(self, event: VmEvent) -> Option<Self> {
        use VmEvent as E;
        use VmState as S;
        match (self, event) {
            (S::Created, E::Configure) => Some(S::Configured),
            (S::Configured, E::Start) | (S::Paused, E::Resume) => Some(S::Running),
            (S::Running, E::Pause) => Some(S::Paused),
            (S::Running | S::Paused, E::Stop) => Some(S::Stopped),
            (S::Configured | S::Running | S::Paused, E::Fail) => Some(S::Failed),
            (S::Created | S::Configured | S::Stopped | S::Failed, E::Destroy) => Some(S::Destroyed),
            _ => None,
        }
    }

    /// Whether no further transition is possible.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Destroyed)
    }
}

/// Tracks the lifecycle of one VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmLifecycle {
    state: VmState,
}

impl Default for VmLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl VmLifecycle {
    /// A new lifecycle in [`VmState::Created`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: VmState::Created,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> VmState {
        self.state
    }

    /// Apply `event`.
    ///
    /// # Errors
    ///
    /// Returns [`LifecycleError`] if `event` is not valid in the current
    /// state; the state is left unchanged.
    pub fn apply(&mut self, event: VmEvent) -> Result<VmState, LifecycleError> {
        let next = self.state.next(event).ok_or(LifecycleError {
            from: self.state,
            event,
        })?;
        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [VmState; 7] = [
        VmState::Created,
        VmState::Configured,
        VmState::Running,
        VmState::Paused,
        VmState::Stopped,
        VmState::Failed,
        VmState::Destroyed,
    ];
    const EVENTS: [VmEvent; 7] = [
        VmEvent::Configure,
        VmEvent::Start,
        VmEvent::Pause,
        VmEvent::Resume,
        VmEvent::Stop,
        VmEvent::Fail,
        VmEvent::Destroy,
    ];

    /// The complete set of allowed transitions. Every pair not listed here
    /// must be rejected.
    const ALLOWED: [(VmState, VmEvent, VmState); 13] = [
        (VmState::Created, VmEvent::Configure, VmState::Configured),
        (VmState::Created, VmEvent::Destroy, VmState::Destroyed),
        (VmState::Configured, VmEvent::Start, VmState::Running),
        (VmState::Configured, VmEvent::Fail, VmState::Failed),
        (VmState::Configured, VmEvent::Destroy, VmState::Destroyed),
        (VmState::Running, VmEvent::Pause, VmState::Paused),
        (VmState::Running, VmEvent::Stop, VmState::Stopped),
        (VmState::Running, VmEvent::Fail, VmState::Failed),
        (VmState::Paused, VmEvent::Resume, VmState::Running),
        (VmState::Paused, VmEvent::Stop, VmState::Stopped),
        (VmState::Paused, VmEvent::Fail, VmState::Failed),
        (VmState::Stopped, VmEvent::Destroy, VmState::Destroyed),
        (VmState::Failed, VmEvent::Destroy, VmState::Destroyed),
    ];

    fn expected(from: VmState, event: VmEvent) -> Option<VmState> {
        ALLOWED
            .iter()
            .find(|(s, e, _)| *s == from && *e == event)
            .map(|(_, _, to)| *to)
    }

    #[test]
    fn every_state_event_pair_matches_the_table() {
        for from in STATES {
            for event in EVENTS {
                assert_eq!(
                    from.next(event),
                    expected(from, event),
                    "{from:?} + {event:?}"
                );
            }
        }
    }

    #[test]
    fn rejected_event_leaves_state_unchanged() {
        let mut vm = VmLifecycle::new();
        let err = vm.apply(VmEvent::Start).unwrap_err();
        assert_eq!(
            err,
            LifecycleError {
                from: VmState::Created,
                event: VmEvent::Start
            }
        );
        assert_eq!(vm.state(), VmState::Created);
    }

    #[test]
    fn normal_run_reaches_destroyed() {
        let mut vm = VmLifecycle::default();
        for event in [
            VmEvent::Configure,
            VmEvent::Start,
            VmEvent::Pause,
            VmEvent::Resume,
            VmEvent::Stop,
            VmEvent::Destroy,
        ] {
            vm.apply(event).unwrap();
        }
        assert!(vm.state().is_terminal());
    }

    #[test]
    fn failure_path_reaches_destroyed() {
        let mut vm = VmLifecycle::new();
        vm.apply(VmEvent::Configure).unwrap();
        vm.apply(VmEvent::Start).unwrap();
        assert_eq!(vm.apply(VmEvent::Fail), Ok(VmState::Failed));
        assert_eq!(vm.apply(VmEvent::Resume).unwrap_err().from, VmState::Failed);
        assert_eq!(vm.apply(VmEvent::Destroy), Ok(VmState::Destroyed));
    }

    #[test]
    fn destroyed_is_terminal_and_rejects_everything() {
        for event in EVENTS {
            assert_eq!(VmState::Destroyed.next(event), None);
        }
        assert!(
            STATES
                .iter()
                .filter(|s| s.is_terminal())
                .eq([VmState::Destroyed].iter())
        );
    }
}
