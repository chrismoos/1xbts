//! Structured gRPC errors for 1xBTS services.
//!
//! A failed RPC returns a `tonic::Status` whose details hold a
//! `google.rpc.ErrorInfo` and, for invalid fields, a `google.rpc.BadRequest`.
//! The `ErrorInfo` reason is an [`ErrorReason`] name and its domain is the
//! service that produced the error, so a client can tell why a call failed and
//! where without parsing the message.

use std::collections::HashMap;

use tonic::{Code, Status};
use tonic_types::{ErrorDetails, StatusExt};

pub mod proto {
    tonic::include_proto!("errors.v1");
}

pub use proto::ErrorReason;

#[derive(Clone, Debug)]
pub struct RpcError {
    code: Code,
    reason: ErrorReason,
    domain: &'static str,
    message: String,
    metadata: HashMap<String, String>,
    fields: Vec<(String, String)>,
}

impl RpcError {
    pub fn new(
        domain: &'static str,
        code: Code,
        reason: ErrorReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            reason,
            domain,
            message: message.into(),
            metadata: HashMap::new(),
            fields: Vec::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl ToString) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    pub fn field(mut self, field: &str, description: impl Into<String>) -> Self {
        self.fields.push((field.to_string(), description.into()));
        self
    }

    pub fn status(self) -> Status {
        let mut details =
            ErrorDetails::with_error_info(self.reason.as_str_name(), self.domain, self.metadata);
        for (field, description) in self.fields {
            details.add_bad_request_violation(field, description);
        }
        Status::with_error_details(self.code, self.message, details)
    }
}

impl From<RpcError> for Status {
    fn from(error: RpcError) -> Self {
        error.status()
    }
}

/// Builds errors for one service. Each service keeps one as a constant, so
/// every error it returns names that service as its domain.
#[derive(Clone, Copy, Debug)]
pub struct Domain(pub &'static str);

impl Domain {
    pub fn error(self, code: Code, reason: ErrorReason, message: impl Into<String>) -> RpcError {
        RpcError::new(self.0, code, reason, message)
    }

    pub fn not_found(self, reason: ErrorReason, message: impl Into<String>) -> RpcError {
        self.error(Code::NotFound, reason, message)
    }

    pub fn precondition(self, reason: ErrorReason, message: impl Into<String>) -> RpcError {
        self.error(Code::FailedPrecondition, reason, message)
    }

    pub fn unavailable(self, reason: ErrorReason, message: impl Into<String>) -> RpcError {
        self.error(Code::Unavailable, reason, message)
    }

    /// An invalid request field. The message is shown to the user, so it
    /// should read as a sentence about the value, not the field name.
    pub fn invalid_field(self, field: &str, message: impl Into<String>) -> Status {
        let message = message.into();
        self.error(
            Code::InvalidArgument,
            ErrorReason::InvalidArgument,
            message.clone(),
        )
        .field(field, message)
        .status()
    }

    pub fn invalid(self, message: impl Into<String>) -> Status {
        self.error(Code::InvalidArgument, ErrorReason::InvalidArgument, message)
            .status()
    }

    pub fn internal(self, message: impl Into<String>) -> Status {
        self.error(Code::Internal, ErrorReason::Internal, message)
            .status()
    }

    pub fn database(self) -> Status {
        self.error(
            Code::Internal,
            ErrorReason::DatabaseUnavailable,
            "The database could not be read or written",
        )
        .status()
    }

    pub fn stopping(self) -> Status {
        self.error(
            Code::Unavailable,
            ErrorReason::ServiceStopping,
            "The service is shutting down",
        )
        .status()
    }

    pub fn not_configured(self, feature: &str, message: impl Into<String>) -> Status {
        self.error(Code::Unavailable, ErrorReason::NotConfigured, message)
            .with("feature", feature)
            .status()
    }

    pub fn not_supported(self, operation: &str, message: impl Into<String>) -> Status {
        self.error(Code::Unimplemented, ErrorReason::NotSupported, message)
            .with("operation", operation)
            .status()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorInfo {
    pub reason: ErrorReason,
    pub domain: String,
    pub metadata: HashMap<String, String>,
}

pub fn error_info(status: &Status) -> Option<ErrorInfo> {
    let details = status.get_error_details();
    let info = details.error_info()?;
    Some(ErrorInfo {
        reason: ErrorReason::from_str_name(&info.reason).unwrap_or(ErrorReason::Unspecified),
        domain: info.domain.clone(),
        metadata: info.metadata.clone(),
    })
}

pub fn field_violations(status: &Status) -> Vec<(String, String)> {
    status
        .get_error_details()
        .bad_request()
        .map(|bad| {
            bad.field_violations
                .iter()
                .map(|violation| (violation.field.clone(), violation.description.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST: Domain = Domain("test");

    #[test]
    fn status_round_trips_reason_domain_and_metadata() {
        let status = TEST
            .precondition(ErrorReason::PrlInUse, "PRL is assigned to 3 subscribers")
            .with("subscriber_count", 3)
            .status();
        assert_eq!(status.code(), Code::FailedPrecondition);
        assert_eq!(status.message(), "PRL is assigned to 3 subscribers");
        let info = error_info(&status).expect("error info");
        assert_eq!(info.reason, ErrorReason::PrlInUse);
        assert_eq!(info.domain, "test");
        assert_eq!(
            info.metadata.get("subscriber_count").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn invalid_field_names_the_field() {
        let status = TEST.invalid_field("imsi", "IMSI must be 15 digits");
        assert_eq!(status.code(), Code::InvalidArgument);
        assert_eq!(
            error_info(&status).map(|info| info.reason),
            Some(ErrorReason::InvalidArgument)
        );
        assert_eq!(
            field_violations(&status),
            vec![("imsi".to_string(), "IMSI must be 15 digits".to_string())]
        );
    }

    #[test]
    fn plain_status_has_no_error_info() {
        assert_eq!(error_info(&Status::internal("boom")), None);
    }
}
