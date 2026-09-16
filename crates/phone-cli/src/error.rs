#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the call requires explicit approval (--approve)")]
    ApprovalRequired,
    #[error("invalid call request: {0}")]
    InvalidRequest(String),
    #[error("invalid phone configuration: {0}")]
    Configuration(String),
    #[error("transcript storage failed: {0}")]
    Storage(String),
    #[error("this call ID already exists; refusing to dial again")]
    DuplicateCall,
    #[error("could not start the calling backend")]
    BackendStart,
    #[error("calling backend exited before reporting a terminal outcome")]
    BackendExited,
    #[error("invalid calling backend protocol: {0}")]
    Protocol(&'static str),
    #[error("could not send cancellation to the calling backend")]
    Cancellation,
    #[error("could not install cancellation handler")]
    SignalHandler,
    #[error("could not write command output")]
    Output,
}
