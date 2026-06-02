//! OTASP (`*228`) session driver, NAM assembly, and event types.
//!
//! Spec: C.S0016-D.

pub mod coordinator;
pub mod event;
pub mod history;
pub mod nam;
pub mod proto_conv;
pub mod session;

pub use coordinator::{OtaspCoordinator, SessionKey};
pub use event::{HardwareIdentity, OtaspEvent, SessionOutcomeKind};
pub use history::{OtaspHistory, OtaspSessionRecord, RecordedEvent};
pub use nam::{AssembledNam, NamReadback, ResolvedSubscriberInput, assemble_nam};
pub use session::{OtaspSession, OtaspTransport, OutboundOtasp, SessionOutcome};
