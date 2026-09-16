use std::time::{Duration, Instant};

use crate::domain::{AuthorizedCall, CallBackend, CallEvent, CallOutcome, TerminationReason};
use crate::error::CallError;
use crate::journal::{Journal, Record};

// Leave room for the worker's in-flight dial RPC and bounded model, transcript,
// room, and client cleanup (47 seconds total), including GPT-Live finalization.
pub const CANCELLATION_GRACE: Duration = Duration::from_secs(60);

enum Phase {
    Starting,
    Ready,
    Dialing,
    Connected,
}

fn unknown_outcome(reason: TerminationReason) -> CallOutcome {
    CallOutcome {
        reason,
        remote_hangup_confirmed: false,
        summary: None,
    }
}

/// A terminal result is only trusted after its event has been durably saved.
/// Errors never trigger a second dial. Cancellation has one bounded grace period.
pub fn run(
    call: &AuthorizedCall,
    backend: &dyn CallBackend,
    journal: &mut Journal,
    cancelled: impl Fn() -> bool,
) -> Result<CallOutcome, CallError> {
    if cancelled() {
        let outcome = CallOutcome {
            reason: TerminationReason::Cancelled,
            remote_hangup_confirmed: true,
            summary: None,
        };
        journal.append(Record::Outcome(outcome.clone()))?;
        return Ok(outcome);
    }
    let mut active = match backend.start(call) {
        Ok(active) => active,
        Err(_) => {
            let outcome = unknown_outcome(TerminationReason::Failed);
            journal.append(Record::Outcome(outcome.clone()))?;
            return Ok(outcome);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(call.request().max_duration_seconds);
    let mut cancellation: Option<(Instant, TerminationReason)> = None;
    let mut phase = Phase::Starting;
    loop {
        let now = Instant::now();
        if cancellation.is_none() {
            let reason = if cancelled() {
                Some(TerminationReason::Cancelled)
            } else if now >= deadline {
                Some(TerminationReason::Timeout)
            } else {
                None
            };
            if let Some(reason) = reason {
                let _ = active.cancel();
                cancellation = Some((now + CANCELLATION_GRACE, reason));
            }
        }
        if let Some((deadline, reason)) = cancellation {
            if now >= deadline {
                let outcome = unknown_outcome(reason);
                journal.append(Record::Outcome(outcome.clone()))?;
                return Ok(outcome);
            }
        }
        let event = match active.next_event(Duration::from_millis(100)) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(_) => {
                let _ = active.cancel();
                let outcome = unknown_outcome(cancellation.map_or(TerminationReason::Failed, |(_, reason)| reason));
                journal.append(Record::Outcome(outcome.clone()))?;
                return Ok(outcome);
            }
        };
        if let Err(error) = journal.append(Record::Event(event.clone())) {
            // Storage failure ends the call too. Allow the backend to hang up,
            // even though the journal can no longer preserve its acknowledgement.
            let _ = active.cancel();
            let stop = Instant::now() + CANCELLATION_GRACE;
            while Instant::now() < stop {
                match active.next_event(Duration::from_millis(100)) {
                    Ok(Some(CallEvent::Completed(_))) | Err(_) => break,
                    _ => {}
                }
            }
            return Err(error);
        }
        match event {
            CallEvent::Completed(mut outcome) => {
                if let Some((_, reason)) = cancellation {
                    outcome.reason = reason;
                } else if outcome.reason == TerminationReason::Completed
                    && !matches!(phase, Phase::Dialing | Phase::Connected)
                {
                    outcome = unknown_outcome(TerminationReason::Failed);
                }
                journal.append(Record::Outcome(outcome.clone()))?;
                return Ok(outcome);
            }
            CallEvent::ApprovalRequired { .. } => {
                if cancellation.is_none() {
                    let _ = active.cancel();
                    cancellation = Some((Instant::now() + CANCELLATION_GRACE, TerminationReason::ApprovalRequired));
                }
            }
            CallEvent::Error { .. } => {
                if cancellation.is_none() {
                    let _ = active.cancel();
                    cancellation = Some((Instant::now() + CANCELLATION_GRACE, TerminationReason::Failed));
                }
            }
            CallEvent::Ready if matches!(phase, Phase::Starting) => phase = Phase::Ready,
            CallEvent::Dialing if matches!(phase, Phase::Ready) => phase = Phase::Dialing,
            CallEvent::Connected if matches!(phase, Phase::Dialing) => phase = Phase::Connected,
            // Early media can contain useful IVR speech before answer metadata.
            CallEvent::Transcript(_) if matches!(phase, Phase::Dialing | Phase::Connected) => {}
            _ => {
                let _ = active.cancel();
                cancellation.get_or_insert((Instant::now() + CANCELLATION_GRACE, TerminationReason::Failed));
            }
        }
    }
}
